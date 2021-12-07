use std::io;

pub fn create_error<T>(str: &str) -> Result<T, io::Error> {
    Err(io::Error::new(io::ErrorKind::Other, str))
}
