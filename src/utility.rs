use std::io::{Error, ErrorKind, Result};

#[derive(Clone, Copy)]
pub enum Action {
    Read(i32, u64),
    Quit,
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
