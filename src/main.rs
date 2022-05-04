use std::collections::VecDeque;
use std::fs::File;
use std::io;
use std::io::Write;

extern crate libc;
use libc::*;

extern crate mio;
use mio::unix::SourceFd;
use mio::{Events, Interest, Poll, Token};

extern crate argparse;
use argparse::{ArgumentParser, Store};

extern crate chrono;
use chrono::Local;

mod uart_tty;
use uart_tty::{Action, UartTty};

mod utility;

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

// Shrink the buffer queues to this size if they ever grow larger.
const BUF_QUEUE_SIZE: usize = 16;

struct PollResults {
    uart_readable: bool,
    uart_writable: bool,
    tty_readable: bool,
    tty_writable: bool,
}

fn get_poll_results(events: &Events) -> PollResults {
    let mut results = PollResults {
        uart_readable: false,
        uart_writable: false,
        tty_readable: false,
        tty_writable: false,
    };
    for event in events {
        if event.token() == STDIN_ID {
            if event.is_readable() {
                results.tty_readable = true;
            }
            if event.is_writable() {
                results.tty_writable = true;
            }
        } else if event.token() == UART_ID {
            if event.is_readable() {
                results.uart_readable = true;
            }
            if event.is_writable() {
                results.uart_writable = true;
            }
        } else {
            panic!("Unknown id in poll!");
        }
    }
    results
}

fn create_logfile() -> Result<File, io::Error> {
    File::create(
        Local::now()
            .format("/home/fionn/Documents/lima-logs/log-%Y-%m-%d_%H:%M:%S")
            .to_string(),
    )
}

fn maybe_write_to<T: FnMut(&[u8]) -> Result<usize, io::Error>>(
    flag: bool,
    queue: &mut VecDeque<Vec<u8>>,
    written_so_far: &mut usize,
    mut write_func: T,
) -> Result<(), io::Error> {
    if flag || queue.len() == 1 {
        loop {
            match queue.front() {
                None => return Ok(()),
                Some(buf) => {
                    // Get slice to remaining bytes to write
                    let buf = buf.get(*written_so_far..).unwrap();
                    match write_func(buf) {
                        Ok(written) => {
                            if written == buf.len() {
                                queue.pop_front();
                                *written_so_far = 0;
                            } else {
                                *written_so_far += written;
                                // partial write, no point in looping
                                return Ok(());
                            }
                        }
                        Err(e) if e.kind() == io::ErrorKind::WouldBlock => return Ok(()),
                        Err(e) => return Err(e),
                    };
                }
            }
        }
    }
    Ok(())
}

// There are 5 parts in mainloop.
//
// First part is creation of Uart object (does uart & tty setup), poll
// & queues.
//
// Second part is first part of actual loop; poll is called and
// results processed.
//
// Third part is read from ready file descriptors, data is buffered in
// queues.
//
// Fourth part is write as much as possible to any writable file
// descriptors; note that all file descriptors are set non-blocking.
//
// Fifth & final part updates the poll set as appropriate.  Loop back
// to second part.
fn mainloop(dev_name: &str) -> Result<(), io::Error> {
    // Part 1
    let mut uart = UartTty::new(dev_name)?;
    let mut poll = Poll::new()?;

    // Kickstart poll loop; we are always ready to read from either
    // source.
    poll.registry()
        .register(&mut SourceFd(&STDIN_FILENO), STDIN_ID, Interest::READABLE)?;
    poll.registry()
        .register(&mut SourceFd(&uart.uart_fd()), UART_ID, Interest::READABLE)?;

    // Two queues are maintained for the buffers that are read; there
    // are no limitations on the size of these queues, but they are
    // shrunk if they are too large (BUF_QUEUE_SIZE).  The current
    // buffer, the buffer at the front of the queue, may be only
    // partly written, this is tracked by the cur_buf_written_to_*
    // variables.
    let mut bufs_to_write_tty = VecDeque::new();
    let mut bufs_to_write_uart = VecDeque::new();
    let mut cur_buf_written_to_tty = 0;
    let mut cur_buf_written_to_uart = 0;

    let mut logfile = create_logfile()?;

    loop {
        // Part 2
        let mut events = Events::with_capacity(2);
        match poll.poll(&mut events, None) {
            Ok(_) => (),
            Err(e) if e.kind() == io::ErrorKind::Interrupted => continue,
            Err(e) => return Err(e),
        };
        let results = get_poll_results(&events);

        // Part 3
        if results.tty_readable {
            match uart.read_from_tty()? {
                Action::AllOk(buf) => bufs_to_write_uart.push_back(buf),
                Action::Quit => return Ok(()),
            }
        }
        if results.uart_readable {
            let buf = uart.read_from_uart()?;
            logfile.write_all(&buf)?;
            bufs_to_write_tty.push_back(buf);
        }

        // Part 4
        maybe_write_to(
            results.tty_writable,
            &mut bufs_to_write_tty,
            &mut cur_buf_written_to_tty,
            |buf| uart.write_to_tty(buf),
        )?;
        maybe_write_to(
            results.uart_writable,
            &mut bufs_to_write_uart,
            &mut cur_buf_written_to_uart,
            |buf| uart.write_to_uart(buf),
        )?;
        bufs_to_write_uart.shrink_to(BUF_QUEUE_SIZE);
        bufs_to_write_tty.shrink_to(BUF_QUEUE_SIZE);

        // Part 5
        poll.registry().reregister(
            &mut SourceFd(&STDIN_FILENO),
            STDIN_ID,
            if bufs_to_write_tty.is_empty() {
                Interest::READABLE
            } else {
                Interest::READABLE | Interest::WRITABLE
            },
        )?;
        poll.registry().reregister(
            &mut SourceFd(&uart.uart_fd()),
            UART_ID,
            if bufs_to_write_uart.is_empty() {
                Interest::READABLE
            } else {
                Interest::READABLE | Interest::WRITABLE
            },
        )?;
    }
}
