use io_uring::types::Fd;
use io_uring::{opcode, IoUring, SubmissionQueue};
use std::collections::HashMap;
use std::fmt::Debug;
use std::fmt::Formatter;

use std::convert::TryInto;
use std::io::{Error, ErrorKind, Result};

pub trait ReactorSubmitter {
    fn submit_read(&mut self, fd: i32, buf: Vec<u8>, callback: RWCallback) -> u64;
    fn submit_write(&mut self, fd: i32, buf: Vec<u8>, offset: usize, callback: RWCallback) -> u64;
    fn submit_cancel(&mut self, id: u64, callback: CancelCallback) -> u64;
}

pub type RWCallback = Box<dyn FnOnce(&mut dyn ReactorSubmitter, i32, Vec<u8>, u64)>;
pub type CancelCallback = Box<dyn FnOnce(&mut dyn ReactorSubmitter, i32, u64)>;

pub struct Submitter {
    actions: Vec<Action>,
    next_id: u64,
}

impl Submitter {
    fn new(next_id: u64) -> Submitter {
        Submitter {
            actions: vec![],
            next_id: next_id,
        }
    }
}

impl ReactorSubmitter for Submitter {
    fn submit_read(&mut self, fd: i32, buf: Vec<u8>, callback: RWCallback) -> u64 {
        let current_id = self.next_id;
        self.next_id += 1;
        self.actions
            .push(Action::Read(fd, buf, current_id, callback));
        return current_id;
    }

    fn submit_write(&mut self, fd: i32, buf: Vec<u8>, offset: usize, callback: RWCallback) -> u64 {
        let current_id = self.next_id;
        self.next_id += 1;
        self.actions
            .push(Action::Write(fd, buf, offset, current_id, callback));
        return current_id;
    }

    fn submit_cancel(&mut self, id: u64, callback: CancelCallback) -> u64 {
        let current_id = self.next_id;
        self.next_id += 1;
        self.actions.push(Action::Cancel(id, current_id, callback));
        return current_id;
    }
}

// #[derive(Debug)]
pub enum Action {
    Read(i32, Vec<u8>, u64, RWCallback),
    Write(i32, Vec<u8>, usize, u64, RWCallback),
    Cancel(u64, u64, CancelCallback),
}

impl Debug for Action {
    fn fmt(&self, fmt: &mut Formatter<'_>) -> std::fmt::Result {
        match self {
            Action::Read(fd, _, user_data, _) => fmt
                .debug_struct("Read")
                .field("fd", fd)
                .field("user_data", user_data)
                .finish(),
            Action::Write(fd, _, offset, user_data, _) => fmt
                .debug_struct("Write")
                .field("fd", fd)
                .field("offset", offset)
                .field("user_data", user_data)
                .finish(),
            Action::Cancel(to_cancel, user_data, _) => fmt
                .debug_struct("Cancel")
                .field("to_cancel", to_cancel)
                .field("user_data", user_data)
                .finish(),
        }
    }
}

fn retry_on_eintr<F, R>(mut fun: F) -> Result<R>
where
    F: FnMut() -> Result<R>,
{
    loop {
        match fun() {
            Err(e) => {
                if e.kind() != ErrorKind::Interrupted {
                    return Err(e);
                }
            }
            x => return x,
        }
    }
}

enum OpInProgress {
    ReadOp(Vec<u8>, RWCallback),
    WriteOp(Vec<u8>, RWCallback),
    OtherOp(CancelCallback),
}

pub struct Reactor {
    in_progress: HashMap<u64, OpInProgress>,
    ring: IoUring,
    next_id: u64,
}

impl Reactor {
    pub fn new(size: u32) -> Result<Reactor> {
        Ok(Reactor {
            in_progress: HashMap::new(),
            ring: IoUring::new(size)?,
            next_id: 1,
        })
    }

    pub fn with_submitter(
        &mut self,
        mut callback: Box<dyn FnMut(&mut Submitter) -> Result<()>>,
    ) -> Result<()> {
        let mut reactor_submitter = Submitter::new(self.next_id);
        let res = callback(&mut reactor_submitter);
        self.next_id += reactor_submitter.next_id;
        for action in reactor_submitter.actions {
            self.submit(action)?;
        }
        return res;
    }

    pub fn run(&mut self) -> Result<()> {
        while !self.in_progress.is_empty() {
            // TODO is there a way for reactor_submitter to work
            // directly on the ring, instead of buffering the actions
            // into another array?
            let mut reactor_submitter = Submitter::new(self.next_id);
            {
                let (submitter, mut submission, mut completion) = self.ring.split();
                submission.sync();
                retry_on_eintr(|| submitter.submit_and_wait(1))?;
                completion.sync();

                for cqe in &mut completion {
                    match self.in_progress.remove(&cqe.user_data()) {
                        None => panic!(
                            "Got user_data in cqe that doesn't exist: {}",
                            cqe.user_data()
                        ),
                        Some(OpInProgress::ReadOp(mut buf, callback)) => {
                            if cqe.result() >= 0 {
                                buf.resize(cqe.result() as usize, 0);
                            } else {
                                buf.clear();
                            }
                            callback(&mut reactor_submitter, cqe.result(), buf, cqe.user_data())
                        }
                        Some(OpInProgress::WriteOp(buf, callback)) => {
                            callback(&mut reactor_submitter, cqe.result(), buf, cqe.user_data())
                        }
                        Some(OpInProgress::OtherOp(callback)) => {
                            callback(&mut reactor_submitter, cqe.result(), cqe.user_data())
                        }
                    };
                }
            }
            self.next_id = reactor_submitter.next_id;
            for action in reactor_submitter.actions {
                self.submit(action)?;
            }
        }
        Ok(())
    }

    fn submit(&mut self, action: Action) -> Result<u64> {
        match action {
            Action::Cancel(op_to_cancel, user_data, callback) => {
                submit_cancel(&mut self.ring.submission(), op_to_cancel, user_data)?;
                if let Some(_) = self
                    .in_progress
                    .insert(user_data, OpInProgress::OtherOp(callback))
                {
                    panic!("user_data {} already registered!", user_data);
                }
                Ok(user_data)
            }
            Action::Read(fd, mut buf, user_data, callback) => {
                submit_read(&mut self.ring.submission(), fd, &mut buf, user_data)?;
                if let Some(_) = self
                    .in_progress
                    .insert(user_data, OpInProgress::ReadOp(buf, callback))
                {
                    panic!("user_data {} already registered!", user_data);
                }
                Ok(user_data)
            }
            Action::Write(fd, mut buf, offset, user_data, callback) => {
                submit_write(&mut self.ring.submission(), fd, &mut buf, offset, user_data)?;
                if let Some(_) = self
                    .in_progress
                    .insert(user_data, OpInProgress::WriteOp(buf, callback))
                {
                    panic!("user_data {} already registered!", user_data);
                }
                Ok(user_data)
            }
        }
    }
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
