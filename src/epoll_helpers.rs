use std::io::Result;
use std::os::unix::io::RawFd;
use std::iter::FromIterator;

extern crate epoll;
use epoll::{EPOLL_CTL_ADD, EPOLL_CTL_MOD, EPOLLIN, EPOLLONESHOT};

use utility::create_error;

pub struct Epoller {
    fd: RawFd,
}

#[derive(Clone, Copy)]
pub enum Timeout {
    Milliseconds(i32),
    Infinite,
}

impl Epoller {
    pub fn new() -> Result<Epoller> {
        let fd = epoll::create(false)?;
        Ok(Epoller { fd })
    }

    pub fn add(&self, fd: RawFd, id: u64) -> Result<()> {
        epoll::ctl(self.fd, EPOLL_CTL_ADD, fd, epoll::Event::new(EPOLLIN, id))?;
        Ok(())
    }

    pub fn add_oneshot(&self, fd: RawFd, id: u64) -> Result<()> {
        epoll::ctl(
            self.fd,
            EPOLL_CTL_ADD,
            fd,
            epoll::Event::new(EPOLLIN | EPOLLONESHOT, id),
        )?;
        Ok(())
    }

    pub fn rearm_oneshot(&self, fd: RawFd, id: u64) -> Result<()> {
        epoll::ctl(
            self.fd,
            EPOLL_CTL_MOD,
            fd,
            epoll::Event::new(EPOLLIN | EPOLLONESHOT, id),
        )?;
        Ok(())
    }

    pub fn wait(&self, timeout: Timeout) -> Result<Vec<u64>> {
        let epoll_timeout: i32 = match timeout {
            Timeout::Infinite => -1 as i32,
            Timeout::Milliseconds(x) => {
                if x < 0 {
                    return create_error("Negative wait specified in Epoller::wait!");
                } else {
                    x
                }
            }
        };
        let mut epoll_events = [empty_epoll_event(); 2];
        let num_events = epoll::wait(self.fd, epoll_timeout, &mut epoll_events)?;
        Ok(Vec::from_iter(
            epoll_events[..num_events].iter().map(|&e| e.data()),
        ))
    }
}

fn empty_epoll_event() -> epoll::Event {
    epoll::Event::new(EPOLLIN, 0)
}
