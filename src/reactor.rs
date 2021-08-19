use io_uring::squeue::Entry;
use io_uring::types::Fd;
use io_uring::{opcode, IoUring};
use std::collections::HashMap;
use std::convert::TryInto;
use std::io::{ErrorKind, Result};

pub type RWCallback = Box<dyn FnOnce(&mut Reactor, i32, Vec<u8>, u64)>;
pub type CancelCallback = Box<dyn FnOnce(&mut Reactor, i32, u64)>;

pub type Op = u64;

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
    submit: io_uring::ownedsplit::SubmitterUring,
    sq: io_uring::ownedsplit::SubmissionUring,
    cq: io_uring::ownedsplit::CompletionUring,
    next_id: u64,
}

impl Reactor {
    pub fn new(size: u32) -> Result<Reactor> {
        let (submitter, submission, completion) = IoUring::new(size)?.owned_split();
        Ok(Reactor {
            in_progress: HashMap::new(),
            submit: submitter,
            sq: submission,
            cq: completion,
            next_id: 1,
        })
    }

    pub fn run(&mut self) -> Result<()> {
        while !self.in_progress.is_empty() {
            self.sq.submission().sync();
            retry_on_eintr(|| self.submit.submitter().submit_and_wait(1))?;
            self.cq.completion().sync();

            loop {
                let next = self.cq.completion().next();
                match next {
                    None => break,
                    Some(cqe) => match self.in_progress.remove(&cqe.user_data()) {
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
                    },
                }
            }
        }
        Ok(())
    }

    pub fn read(&mut self, fd: i32, mut buf: Vec<u8>, callback: RWCallback) -> Op {
        self.submit_entry(
            opcode::Read::new(Fd(fd), buf.as_mut_ptr(), buf.len().try_into().unwrap()).build(),
            OpInProgress::ReadOp(buf, callback),
        )
    }

    pub fn write(&mut self, fd: i32, mut buf: Vec<u8>, offset: usize, callback: RWCallback) -> Op {
        self.submit_entry(
            opcode::Write::new(Fd(fd), buf.as_mut_ptr(), buf.len().try_into().unwrap())
                .offset(offset.try_into().unwrap())
                .build(),
            OpInProgress::WriteOp(buf, callback),
        )
    }

    pub fn cancel(&mut self, op_to_cancel: Op, callback: CancelCallback) -> Op {
        self.submit_entry(
            opcode::AsyncCancel::new(op_to_cancel).build(),
            OpInProgress::OtherOp(callback),
        )
    }

    fn submit_entry(&mut self, mut entry: Entry, op: OpInProgress) -> Op {
        let user_data = self.next_id;
        self.next_id += 1;
        entry = entry
            .flags(io_uring::squeue::Flags::ASYNC)
            .user_data(user_data);
        loop {
            let res = unsafe { self.sq.submission().push(&entry) };
            match res {
                Ok(_) => break,
                Err(_) => {
                    // Queue full, submit and repush
                    self.sq.submission().sync();
                    // TODO don't ignore this error
                    retry_on_eintr(|| self.submit.submitter().submit()).unwrap();
                }
            }
        }
        if let Some(_) = self.in_progress.insert(user_data, op) {
            panic!("user_data {} already registered!", user_data);
        }
        user_data
    }
}
