use std::io::{Error, ErrorKind, Result};

pub const TTY_READ: u64 = 1;
pub const UART_READ: u64 = 2;
pub const TTY_WRITE: u64 = 3;
pub const UART_WRITE: u64 = 4;

pub const TTY_READ_CANCEL: u64 = 5;
pub const UART_READ_CANCEL: u64 = 6;
pub const TTY_WRITE_CANCEL: u64 = 7;
pub const UART_WRITE_CANCEL: u64 = 8;

pub const TRANSCRIPT_FLUSH: u64 = 9;

#[derive(Debug)]
pub enum Action {
    Read(i32, Vec<u8>, u64),
    Write(i32, Vec<u8>, usize, u64),
    Cancel(u64, u64),
}

pub fn create_error<T>(str: &str) -> Result<T> {
    Err(Error::new(ErrorKind::Other, str))
}

pub fn retry_on_eintr<F, R>(mut fun: F) -> Result<R>
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

#[cfg(test)]
pub fn check_write(
    action: &Action,
    expected_fd: i32,
    buf_len: usize,
    expected_offset: usize,
    expected_user_data: u64,
) {
    match &action {
        Action::Write(fd, buf, offset, user_data) => {
            assert_eq!(*fd, expected_fd);
            assert_eq!(buf.len(), buf_len);
            assert_eq!(*offset, expected_offset);
            assert_eq!(*user_data, expected_user_data);
        }
        _ => assert!(false),
    };
}
