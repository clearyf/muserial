use std::io::{Error, ErrorKind, Result};

extern crate libc;
use libc::*;

extern crate io_uring;
use io_uring::types::Fd;
use io_uring::{opcode, IoUring, SubmissionQueue};

extern crate argparse;
use argparse::{ArgumentParser, Store};

extern crate chrono;

mod uart_tty;
use uart_tty::UartTty;

mod utility;
use utility::{retry_on_eintr, Action};

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
        Err(why) => println!("\nError: {}", why),
    }
}

fn mainloop(dev_name: &str) -> Result<()> {
    let mut ring = IoUring::new(4)?;
    let (submitter, mut submission, mut completion) = ring.split();
    let mut uart = UartTty::new(dev_name)?;

    for (fd, user_data) in uart.init_reads() {
        submit_pollin(&mut submission, fd, user_data)?;
    }
    loop {
        submission.sync();
        retry_on_eintr(|| submitter.submit_and_wait(1))?;
        completion.sync();
        for cqe in &mut completion {
            match uart.handle_read(cqe.result(), cqe.user_data())? {
                Action::Read(fd, user_data) => {
                    submit_pollin(&mut submission, fd, user_data)?;
                }
                Action::Quit => return Ok(()),
            }
        }
    }
}

fn submit_pollin(sq: &mut SubmissionQueue, fd: i32, user_data: u64) -> Result<()> {
    let entry = opcode::PollAdd::new(Fd(fd), POLLIN as _)
        .build()
        .user_data(user_data);
    unsafe { sq.push(&entry) }
        .map_err(|e| Error::new(ErrorKind::Other, format!("io-uring push error: {}", e)))
}
