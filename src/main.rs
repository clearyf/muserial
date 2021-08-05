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
use uart_tty::UartTtySM;

mod transcript;
use transcript::Transcript;

mod utility;
use utility::retry_on_eintr;
use utility::Action;

fn main() {
    let mut dev_name = String::from("/dev/ttyUSB0");
    {
        let mut ap = ArgumentParser::new();
        ap.set_description("Connect to a serial line.");
        ap.refer(&mut dev_name)
            .add_argument("tty-device", Store, "Tty device to connect to");
        ap.parse_args_or_exit();
    }
    println!("Opening uart: {}", dev_name);

    match mainloop(&dev_name) {
        Ok(()) => println!("\r"),
        Err(why) => println!("\nError: {}", why),
    }
}

enum OpInProgress {
    ReadOp(Vec<u8>),
    WriteOp(Vec<u8>),
    OtherOp,
}

fn mainloop(dev_name: &str) -> Result<()> {
    let mut ring = IoUring::new(4)?;
    let mut in_progress: HashMap<u64, OpInProgress> = HashMap::new();
    let (submitter, mut submission, mut completion) = ring.split();
    let uart = UartTty::new(dev_name)?;
    let mut sm = UartTtySM::new(uart.uart_fd(), Transcript::new().ok());

    handle_actions(&mut submission, &mut in_progress, sm.init_actions())?;
    while !in_progress.is_empty() {
        submission.sync();
        retry_on_eintr(|| submitter.submit_and_wait(1))?;
        completion.sync();
        for cqe in &mut completion {
            let actions = match in_progress.remove(&cqe.user_data()) {
                None => panic!(
                    "Got user_data in cqe that doesn't exist: {}",
                    cqe.user_data()
                ),
                Some(OpInProgress::ReadOp(mut buf)) => {
                    if cqe.result() >= 0 {
                        buf.resize(cqe.result() as usize, 0);
                    } else {
                        buf.clear();
                    }
                    sm.handle_buffer_ev(cqe.result(), buf, cqe.user_data())?
                }
                Some(OpInProgress::WriteOp(buf)) => {
                    sm.handle_buffer_ev(cqe.result(), buf, cqe.user_data())?
                }
                Some(OpInProgress::OtherOp) => sm.handle_other_ev(cqe.result(), cqe.user_data())?,
            };
            handle_actions(&mut submission, &mut in_progress, actions)?;
        }
    }
    Ok(())
}

fn handle_actions(
    submission: &mut SubmissionQueue,
    buffers: &mut HashMap<u64, OpInProgress>,
    actions: Vec<Action>,
) -> Result<usize> {
    let mut count = 0;
    for action in actions {
        match action {
            Action::Cancel(op_to_cancel, user_data) => {
                submit_cancel(submission, op_to_cancel, user_data)?;
                if let Some(_) = buffers.insert(user_data, OpInProgress::OtherOp) {
                    panic!("user_data {} already registered!", user_data);
                }
            }
            Action::Read(fd, mut buf, user_data) => {
                submit_read(submission, fd, &mut buf, user_data)?;
                if let Some(_) = buffers.insert(user_data, OpInProgress::ReadOp(buf)) {
                    panic!("user_data {} already registered!", user_data);
                }
            }
            Action::Write(fd, mut buf, offset, user_data) => {
                submit_write(submission, fd, &mut buf, offset, user_data)?;
                if let Some(_) = buffers.insert(user_data, OpInProgress::WriteOp(buf)) {
                    panic!("user_data {} already registered!", user_data);
                }
            }
        }
        count += 1;
    }
    Ok(count)
}

fn submit_read(sq: &mut SubmissionQueue, fd: i32, buf: &mut [u8], user_data: u64) -> Result<()> {
    let entry = opcode::Read::new(Fd(fd), buf.as_mut_ptr(), buf.len().try_into().unwrap())
        .build()
        .flags(io_uring::squeue::Flags::ASYNC)
        .user_data(user_data);
    unsafe { sq.push(&entry) }
        .map_err(|e| Error::new(ErrorKind::Other, format!("io-uring push error: {}", e)))
}

fn submit_write(
    sq: &mut SubmissionQueue,
    fd: i32,
    buf: &mut [u8],
    offset: usize,
    user_data: u64,
) -> Result<()> {
    let entry = opcode::Write::new(Fd(fd), buf.as_mut_ptr(), buf.len().try_into().unwrap())
        .offset(offset.try_into().unwrap())
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
        .user_data(user_data);
    unsafe { sq.push(&entry) }
        .map_err(|e| Error::new(ErrorKind::Other, format!("io-uring push error: {}", e)))
}
