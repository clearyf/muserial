use std::collections::HashMap;
use std::convert::TryInto;
use std::io::{Error, ErrorKind, Result};

extern crate libc;

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
        Ok(()) => println!("\nExiting"),
        Err(why) => println!("\nError: {}", why),
    }
}

fn mainloop(dev_name: &str) -> Result<()> {
    let mut ring = IoUring::new(4)?;
    let mut buffers: HashMap<u64, Vec<u8>> = HashMap::new();
    let (submitter, mut submission, mut completion) = ring.split();
    let mut uart = UartTty::new(dev_name)?;
    let mut num_in_flight = 0;

    for action in uart.init_actions() {
        num_in_flight += handle_action(&mut submission, &mut buffers, action)?;
    }
    while num_in_flight > 0 {
        submission.sync();
        retry_on_eintr(|| submitter.submit_and_wait(1))?;
        completion.sync();
        for cqe in &mut completion {
            num_in_flight -= 1;
            let action = if let Some(buffer) = buffers.remove(&cqe.user_data()) {
                uart.handle_buffer(cqe.result(), buffer, cqe.user_data())?
            } else {
                uart.handle_poll(cqe.result(), cqe.user_data())?
            };
            num_in_flight += handle_action(&mut submission, &mut buffers, action)?;
        }
    }
    Ok(())
}

fn handle_action(
    submission: &mut SubmissionQueue,
    buffers: &mut HashMap<u64, Vec<u8>>,
    action: Action,
) -> Result<usize> {
    match action {
        Action::NoOp => {
            return Ok(0);
        }
        Action::Cancel(op_to_cancel, user_data) => {
            submit_cancel(submission, op_to_cancel, user_data)?;
        }
        Action::Read(fd, mut buf, user_data) => {
            submit_read(submission, fd, &mut buf, user_data)?;
            if let Some(_old_buf) = buffers.insert(user_data, buf) {
                panic!("user_data {} already registered!", user_data);
            }
        }
        Action::Write(fd, mut buf, user_data) => {
            submit_write(submission, fd, &mut buf, user_data)?;
            if let Some(_old_buf) = buffers.insert(user_data, buf) {
                panic!("user_data {} already registered!", user_data);
            }
        }
    }
    Ok(1)
}

fn submit_read(sq: &mut SubmissionQueue, fd: i32, buf: &mut [u8], user_data: u64) -> Result<()> {
    let entry = opcode::Read::new(Fd(fd), buf.as_mut_ptr(), buf.len().try_into().unwrap())
        .build()
        .flags(io_uring::squeue::Flags::ASYNC)
        .user_data(user_data);
    unsafe { sq.push(&entry) }
        .map_err(|e| Error::new(ErrorKind::Other, format!("io-uring push error: {}", e)))
}

fn submit_write(sq: &mut SubmissionQueue, fd: i32, buf: &mut [u8], user_data: u64) -> Result<()> {
    let entry = opcode::Write::new(Fd(fd), buf.as_mut_ptr(), buf.len().try_into().unwrap())
        .build()
        .flags(io_uring::squeue::Flags::ASYNC)
        .user_data(user_data);
    unsafe { sq.push(&entry) }
        .map_err(|e| Error::new(ErrorKind::Other, format!("io-uring push error: {}", e)))
}

fn submit_cancel(sq: &mut SubmissionQueue, op_to_cancel: u64, user_data: u64) -> Result<()> {
    let entry = opcode::AsyncCancel::new(op_to_cancel)
        .build()
        .flags(io_uring::squeue::Flags::ASYNC)
        .user_data(user_data)
        ;
    unsafe { sq.push(&entry) }
        .map_err(|e| Error::new(ErrorKind::Other, format!("io-uring push error: {}", e)))
}
