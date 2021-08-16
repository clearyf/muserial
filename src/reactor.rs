use io_uring::types::Fd;
use io_uring::{opcode, IoUring};
use std::collections::HashMap;

use std::convert::TryInto;
use std::io::{ErrorKind, Result};

pub trait ReactorSubmitter {
    fn submit_read(&mut self, fd: i32, buf: Vec<u8>, callback: RWCallback) -> u64;
    fn submit_write(&mut self, fd: i32, buf: Vec<u8>, offset: usize, callback: RWCallback) -> u64;
    fn submit_cancel(&mut self, id: u64, callback: CancelCallback) -> u64;
}

pub type RWCallback = Box<dyn FnOnce(&mut dyn ReactorSubmitter, i32, Vec<u8>, u64)>;
pub type CancelCallback = Box<dyn FnOnce(&mut dyn ReactorSubmitter, i32, u64)>;

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
    submitter: io_uring::ownedsplit::SubmitterUring,
    submission: io_uring::ownedsplit::SubmissionUring,
    completion: io_uring::ownedsplit::CompletionUring,
    next_id: u64,
}

impl Reactor {
    pub fn new(size: u32) -> Result<Reactor> {
        let (submitter, submission, completion) = IoUring::new(size)?.owned_split();
        Ok(Reactor {
            in_progress: HashMap::new(),
            submitter: submitter,
            submission: submission,
            completion: completion,
            next_id: 1,
        })
    }

    pub fn with_submitter(
        &mut self,
        mut callback: Box<dyn FnMut(&mut Reactor) -> Result<()>>,
    ) -> Result<()> {
        callback(self)
    }

    pub fn run(&mut self) -> Result<()> {
        while !self.in_progress.is_empty() {
            self.submission.submission().sync();
            retry_on_eintr(|| self.submitter.submitter().submit_and_wait(1))?;
            self.completion.completion().sync();

            loop {
                let next = self.completion.completion().next();
                match next {
                    None => break,
                    Some(cqe) => {
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
                                callback(self, cqe.result(), buf, cqe.user_data());
                            }
                            Some(OpInProgress::WriteOp(buf, callback)) => {
                                callback(self, cqe.result(), buf, cqe.user_data());
                            }
                            Some(OpInProgress::OtherOp(callback)) => {
                                callback(self, cqe.result(), cqe.user_data());
                            }
                        }
                    }
                }
            };
        }
        Ok(())
    }
}

impl ReactorSubmitter for Reactor {
    fn submit_read(&mut self, fd: i32, mut buf: Vec<u8>, callback: RWCallback) -> u64 {
        let user_data = self.next_id;
        self.next_id += 1;
        let entry = opcode::Read::new(Fd(fd), buf.as_mut_ptr(), buf.len().try_into().unwrap())
            .build()
            .flags(io_uring::squeue::Flags::ASYNC)
            .user_data(user_data);
        match unsafe { self.submission.submission().push(&entry) } {
            Ok(_) => (),
            Err(e) => panic!("io-uring push error: {}", e),
        }
        if let Some(_) = self
            .in_progress
            .insert(user_data, OpInProgress::ReadOp(buf, callback))
        {
            panic!("user_data {} already registered!", user_data);
        }
        user_data
    }

    fn submit_write(
        &mut self,
        fd: i32,
        mut buf: Vec<u8>,
        offset: usize,
        callback: RWCallback) -> u64 {
        let user_data = self.next_id;
        self.next_id += 1;
        let entry = opcode::Write::new(Fd(fd), buf.as_mut_ptr(), buf.len().try_into().unwrap())
            .offset(offset.try_into().unwrap())
            .build()
            .flags(io_uring::squeue::Flags::ASYNC)
            .user_data(user_data);
        match unsafe { self.submission.submission().push(&entry) } {
            Ok(_) => (),
            Err(e) => panic!("io-uring push error: {}", e),
        }
        if let Some(_) = self
            .in_progress
            .insert(user_data, OpInProgress::WriteOp(buf, callback))
        {
            panic!("user_data {} already registered!", user_data);
        }
        user_data
    }

    fn submit_cancel(&mut self, op_to_cancel: u64, callback: CancelCallback) -> u64 {
        let user_data = self.next_id;
        self.next_id += 1;
        let entry = opcode::AsyncCancel::new(op_to_cancel)
            .build()
            .flags(io_uring::squeue::Flags::ASYNC)
            .user_data(user_data);
        match unsafe { self.submission.submission().push(&entry) } {
            Ok(_) => (),
            Err(e) => panic!("io-uring push error: {}", e),
        }
        if let Some(_) = self
            .in_progress
            .insert(user_data, OpInProgress::OtherOp(callback))
        {
            panic!("user_data {} already registered!", user_data);
        }
        user_data
    }

}
