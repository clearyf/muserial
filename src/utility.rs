use std::io::{Error, ErrorKind, Result};

#[cfg(testDisabled)]
use crate::reactor::Action;

pub fn create_error<T>(str: &str) -> Result<T> {
    Err(Error::new(ErrorKind::Other, str))
}

#[cfg(testDisabled)]
pub fn check_write(action: &Action, expected_fd: i32, buf_len: usize, expected_offset: usize) {
    match &action {
        Action::Write(fd, buf, offset, _, _) => {
            assert_eq!(*fd, expected_fd);
            assert_eq!(buf.len(), buf_len);
            assert_eq!(*offset, expected_offset);
        }
        _ => assert!(false),
    };
}
