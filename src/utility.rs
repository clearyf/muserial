use std::io::{Error, ErrorKind, Result};

pub enum Action {
    Read(i32, Vec<u8>, u64),
    Write(i32, Vec<u8>, u64),
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
