use std::collections::VecDeque;
use std::fs::File;
use std::io;
use std::io::{BufWriter, Write};
use std::process::Command;

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

// Shrink the buffer queues to this size if they ever grow larger.
const BUF_QUEUE_SIZE: usize = 4;

// The size of the buffers in the queue.
const MAX_BUF_SIZE: usize = 4096;

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

struct Logfile {
    handle: BufWriter<File>,
    path: String,
}

impl Logfile {
    fn new() -> Result<Logfile, io::Error> {
        let p = Local::now()
            .format("/home/fionn/Documents/lima-logs/log-%Y-%m-%d_%H:%M:%S")
            .to_string();
        Ok(Logfile {
            handle: BufWriter::new(File::create(&p)?),
            path: p,
        })
    }

    fn log(&mut self, buf: &[u8]) -> Result<(), io::Error> {
        self.handle.write_all(buf)
    }
}

impl Drop for Logfile {
    fn drop(&mut self) {
        if let Err(e) = self.handle.flush() {
            eprintln!("Got {} when trying to flush logfile {}", e, self.path);
        }
        match Command::new("xz").arg(&self.path).status() {
            Ok(status) => {
                if !status.success() {
                    eprintln!("Got {} on running xz on {}", status, self.path);
                }
            }
            Err(e) => {
                eprintln!("Got an error {} trying to run xz on {}", e, self.path);
            }
        }
    }
}

fn flush_queue<T: FnMut(&[u8]) -> Result<usize, io::Error>>(
    queue: &mut VecDeque<Vec<u8>>,
    written_so_far: &mut usize,
    mut write_func: T,
) -> Result<(), io::Error> {
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
                            // anymore as this should only happen
                            // if there wasn't enough room in the
                            // receving buffer.
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

fn enqueue_buf(buf: &[u8], queue: &mut VecDeque<Vec<u8>>) {
    match queue.front_mut() {
        None => queue.push_back(buf.to_vec()),
        Some(front_buf) if front_buf.len() + buf.len() < MAX_BUF_SIZE => front_buf.extend(buf),
        Some(_) => queue.push_back(buf.to_vec()),
    };
}

fn try_write_or_enqueue<T: FnMut(&[u8]) -> Result<usize, io::Error>>(
    buf: &[u8],
    queue: &mut VecDeque<Vec<u8>>,
    mut write_func: T,
) -> Result<(), io::Error> {
    if !queue.is_empty() {
        enqueue_buf(buf, queue);
        return Ok(());
    }

    match write_func(buf) {
        Ok(written) if written == buf.len() => Ok(()),
        Ok(written) => {
            // Partial write
            enqueue_buf(&buf[written..], queue);
            Ok(())
        }
        Err(e) if e.kind() == io::ErrorKind::WouldBlock => {
            // Try again later
            enqueue_buf(buf, queue);
            Ok(())
        }
        Err(e) => Err(e),
    }
}

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
// First part is creation of Uart object (does uart & tty setup), poll
// & queues.
//
// Second part is first part of actual loop; poll is called and
// results processed.
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

    let mut buf = vec![0; BUFFER_SIZE];
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
            buf.resize(BUFFER_SIZE, 0);
            match uart.read_from_tty(&mut buf)? {
                Action::AllOk => {
                    try_write_or_enqueue(&buf, &mut bufs_to_write_uart, |b| uart.write_to_uart(b))?
                }
                Action::Quit => return Ok(()),
            };
        }
        if results.uart_readable {
            buf.resize(BUFFER_SIZE, 0);
            uart.read_from_uart(&mut buf)?;
            logfile.log(&buf)?;
            try_write_or_enqueue(&buf, &mut bufs_to_write_tty, |b| uart.write_to_tty(b))?;
        }

        // Part 4
        if results.tty_writable {
            flush_queue(&mut bufs_to_write_tty, &mut cur_buf_written_to_tty, |b| {
                uart.write_to_tty(b)
            })?;
        }
        if results.uart_writable {
            flush_queue(&mut bufs_to_write_uart, &mut cur_buf_written_to_uart, |b| {
                uart.write_to_uart(b)
            })?;
        }
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
