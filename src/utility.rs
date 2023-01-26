use std::io::{Error, ErrorKind, Result};

pub fn create_error<T>(str: &str) -> Result<T> {
    Err(Error::new(ErrorKind::Other, str))
}
