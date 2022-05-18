use std::io;
use std::io::Read;

extern crate libc;
use libc::*;

extern crate mio;
use mio::unix::SourceFd;
use mio::{Events, Interest, Poll, Token};

extern crate argparse;
use argparse::{ArgumentParser, Store};

extern crate chrono;

mod uart_tty;
use uart_tty::UartTty;

mod utility;
use utility::create_error;

mod logfile;
use logfile::Logfile;

mod bufqueue;
use bufqueue::BufQueue;

// The read buffer is this size, but the same buffer is always reused.
const BUFFER_SIZE: usize = 4096;

// The actual interesting code is in the mainloop function
fn main() {
    let mut dev_name = "/dev/ttyUSB0".to_string();
    {
        let mut ap = ArgumentParser::new();
        ap.set_description("Connect to a serial line.");
        ap.refer(&mut dev_name)
            .add_argument("tty-device", Store, "Tty device to connect to");
        ap.parse_args_or_exit();
    }
    println!("Opening uart: {}", dev_name);

    match mainloop(&dev_name) {
        Ok(()) => println!("\nExiting on request"),
        Err(why) => println!("\nExiting due to error: {}", why),
    }
}

const STDIN_ID: Token = Token(0);
const UART_ID: Token = Token(1);

// This is quite different to how I have structured this in the past.
// Previously the poll loop waited for the file descriptors to become
// ready, read from them and then wrote that data synchronously.  The
// problem here was that sometimes I would "stop" the terminal, either
// using Ctrl-s/Ctrl-q or by running inside screen and using the
// "copy" function.  So now this works by never blocking, and instead
// if data cannot be written then it queues it here, until the data
// can be written again.
//
// There are 5 parts in mainloop.
//
// First part is creation of UartTty object (does uart & tty setup),
// poll object & buffer queues.
//
// Second part is first part of actual loop; poll is called and waits
// indefinitely.
//
// Third part is read from ready file descriptors, attempt to write
// immediately if no data is currently waiting to send, otherwise
// buffer in the queues.
//
// Fourth part is write as much as possible to any writable file
// descriptors; note that all file descriptors are set non-blocking.
//
// Fifth & final part updates the poll set as appropriate.  Loop back
// to second part.
fn mainloop(dev_name: &str) -> Result<(), io::Error> {
    // Part 1
    let mut logfile = Logfile::new()?;

    let mut uart_tty = UartTty::new(dev_name)?;
    let mut poll = Poll::new()?;

    // Kickstart poll loop; we are always ready to read from either
    // source.
    poll.registry()
        .register(&mut SourceFd(&STDIN_FILENO), STDIN_ID, Interest::READABLE)?;
    poll.registry().register(
        &mut SourceFd(&uart_tty.uart_fd()),
        UART_ID,
        Interest::READABLE,
    )?;

    // Two queues are maintained for the buffers that are read.
    let mut bufs_to_write_to_tty = BufQueue::new();
    let mut bufs_to_write_to_uart = BufQueue::new();

    let mut buf = vec![0; BUFFER_SIZE];
    loop {
        // Part 2
        let mut events = Events::with_capacity(2);
        match poll.poll(&mut events, None) {
            Ok(_) => (),
            Err(e) if e.kind() == io::ErrorKind::Interrupted => continue,
            Err(e) => return Err(e),
        };

        // Part 3
        if events.iter().any(|ev| ev.is_readable() && ev.token() == STDIN_ID) {
            let read_size = uart_tty.tty().read(&mut buf)?;
            if read_size == 0 {
                return create_error("Got EOF on local tty");
            }

            let control_o: u8 = 0x0f;
            if buf[..read_size].contains(&control_o) {
                return Ok(());
            }
            bufs_to_write_to_uart.try_write_or_enqueue(&buf[..read_size], uart_tty.uart())?;
        }
        if events.iter().any(|ev| ev.is_readable() && ev.token() == UART_ID) {
            let read_size = uart_tty.uart().read(&mut buf)?;
            if read_size == 0 {
                return create_error("Remote UART disconnected");
            }
            logfile.log(&buf[..read_size])?;
            bufs_to_write_to_tty.try_write_or_enqueue(&buf[..read_size], uart_tty.tty())?;
        }

        // Part 4
        if events.iter().any(|ev| ev.is_writable() && ev.token() == STDIN_ID) {
            bufs_to_write_to_tty.flush(uart_tty.tty())?;
        }
        if events.iter().any(|ev| ev.is_writable() && ev.token() == UART_ID) {
            bufs_to_write_to_uart.flush(uart_tty.uart())?;
        }

        // Part 5
        poll.registry().reregister(
            &mut SourceFd(&STDIN_FILENO),
            STDIN_ID,
            if bufs_to_write_to_tty.is_empty() {
                Interest::READABLE
            } else {
                Interest::READABLE | Interest::WRITABLE
            },
        )?;
        poll.registry().reregister(
            &mut SourceFd(&uart_tty.uart_fd()),
            UART_ID,
            if bufs_to_write_to_uart.is_empty() {
                Interest::READABLE
            } else {
                Interest::READABLE | Interest::WRITABLE
            },
        )?;
    }
}
