use std::collections::HashMap;
use std::convert::TryInto;
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

enum Todo {
    Nothing,
    Quit,
}

fn mainloop(dev_name: &str) -> Result<()> {
    let mut ring = IoUring::new(4)?;
    let mut buffers: HashMap<u64, Vec<u8>> = HashMap::new();
    let (submitter, mut submission, mut completion) = ring.split();
    let mut uart = UartTty::new(dev_name)?;

    for action in uart.init_actions() {
        handle_action(&mut submission, &mut buffers, &action)?;
    }
    loop {
        submission.sync();
        retry_on_eintr(|| submitter.submit_and_wait(1))?;
        completion.sync();
        for cqe in &mut completion {
            let action = if let Some(buffer) = buffers.remove(&cqe.user_data()) {
                uart.handle_buffer(cqe.result(), buffer, cqe.user_data())?
            } else {
                uart.handle_poll(cqe.result(), cqe.user_data())?
            };
            if let Todo::Quit = handle_action(&mut submission, &mut buffers, &action)? {
                return Ok(());
            }
        }
    }
}

fn handle_action(
    submission: &mut SubmissionQueue,
    buffers: &mut HashMap<u64, Vec<u8>>,
    action: &Action,
) -> Result<Todo> {
    match action {
        Action::PollIn(fd, user_data) => {
            submit_pollin(submission, *fd, *user_data)?;
        }
        Action::Read(fd, size, user_data) => {
            if let Some(_old_value) = buffers.insert(*user_data, vec![0; *size]) {
                panic!("user_data {} already WIP!", user_data);
            }
            let buf = buffers.get_mut(&user_data).unwrap();
            submit_read(submission, *fd, buf, *user_data)?;
        }
        Action::Quit => return Ok(Todo::Quit),
    }
    Ok(Todo::Nothing)
}

fn submit_pollin(sq: &mut SubmissionQueue, fd: i32, user_data: u64) -> Result<()> {
    let entry = opcode::PollAdd::new(Fd(fd), POLLIN as _)
        .build()
        .flags(io_uring::squeue::Flags::ASYNC)
        .user_data(user_data);
    unsafe { sq.push(&entry) }
        .map_err(|e| Error::new(ErrorKind::Other, format!("io-uring push error: {}", e)))
}

fn submit_read(sq: &mut SubmissionQueue, fd: i32, buf: &mut [u8], user_data: u64) -> Result<()> {
    let entry = opcode::Read::new(Fd(fd), buf.as_mut_ptr(), buf.len().try_into().unwrap())
        .build()
        .flags(io_uring::squeue::Flags::ASYNC)
        .user_data(user_data);
    unsafe { sq.push(&entry) }
        .map_err(|e| Error::new(ErrorKind::Other, format!("io-uring push error: {}", e)))
}
