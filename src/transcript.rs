use std::collections::VecDeque;
use std::env;
use std::fs::File;
use std::io::{Error, ErrorKind, Result};
use std::os::unix::io::AsRawFd;
use std::process::Command;

use utility::*;

const TRANSCRIPT_BUFFER_SIZE: usize = 4096;

pub struct Transcript {
    path: String,
    file: File,
    offset: usize,
    current_buf: Vec<u8>,
    flushing: bool,
    bufs_to_be_flushed: VecDeque<Vec<u8>>,
    compress_file: bool,
}

impl Transcript {
    #[cfg(test)]
    fn use_existing_file(file: File, path: String) -> Result<Transcript> {
        Ok(Transcript {
            file: file,
            path: path,
            offset: 0,
            current_buf: Vec::with_capacity(TRANSCRIPT_BUFFER_SIZE),
            flushing: false,
            bufs_to_be_flushed: VecDeque::new(),
            compress_file: false,
        })
    }

    pub fn new() -> Result<Transcript> {
        match get_transcript() {
            Ok((file, path)) => {
                println!("\r\nOpened transcript: {}\r", path);
                Ok(Transcript {
                    path: path,
                    file: file,
                    offset: 0,
                    current_buf: Vec::with_capacity(TRANSCRIPT_BUFFER_SIZE),
                    flushing: false,
                    bufs_to_be_flushed: VecDeque::new(),
                    compress_file: true,
                })
            }
            Err(e) => {
                println!("\r\nCouldn't open transcript: {}\r", e);
                Err(e)
            }
        }
    }

    pub fn handle_buffer_ev(
        &mut self,
        result: i32,
        mut buf: Vec<u8>,
        user_data: u64,
    ) -> Result<Vec<Action>> {
        assert!(self.flushing);
        if user_data != TRANSCRIPT_FLUSH {
            panic!(
                "Got unexpected user_data {} in Transcript::handle_buffer_ev",
                user_data
            );
        }
        if result == 0 {
            panic!("Got EOF on write in Transcript::handle_buffer_ev");
        }
        if result < 0 {
            // WTF can be done?
            panic!(
                "Got error on write in Transcript::handle_buffer_ev: {}",
                result
            );
        }
        self.offset += result as usize;
        if (result as usize) < buf.len() {
            // short write
            let new_buf = buf.split_off(result as usize);
            return Ok(vec![self.write_buf(new_buf)]);
        }
        // Queued buffer done, check for next
        if let Some(next_buf) = self.bufs_to_be_flushed.pop_front() {
            return Ok(vec![self.write_buf(next_buf)]);
        }
        // Nothing more to do for the moment
        self.flushing = false;
        Ok(vec![])
    }

    pub fn start_teardown(&mut self) -> Option<Action> {
        if self.flushing {
            // Nothing to do right now, will continue flushing in the
            // background
            return None;
        }
        assert!(self.bufs_to_be_flushed.is_empty());
        if self.current_buf.is_empty() {
            // Nothing more to do
            return None;
        }
        self.flushing = true;
        let mut new_buf = Vec::with_capacity(TRANSCRIPT_BUFFER_SIZE);
        std::mem::swap(&mut new_buf, &mut self.current_buf);
        Some(self.write_buf(new_buf))
    }

    pub fn log(&mut self, buf: &[u8]) -> Option<Action> {
        // Calculate new size of the buffer; if it would be larger
        // than the reserved size then copy the current buffer to a
        // new vec, and start flushing the current buffer.  I'm hoping
        // that normally the buffers being logged are <<< than the
        // TRANSCRIPT_BUFFER_SIZE, so most buffers flushed should be
        // fairly close to TRANSCRIPT_BUFFER_SIZE.
        let new_buf_size = buf.len() + self.current_buf.len();
        if new_buf_size < TRANSCRIPT_BUFFER_SIZE {
            self.current_buf.extend(buf);
            return None;
        }
        let mut buf_to_flush = Vec::with_capacity(TRANSCRIPT_BUFFER_SIZE);
        std::mem::swap(&mut self.current_buf, &mut buf_to_flush);
        if new_buf_size == TRANSCRIPT_BUFFER_SIZE {
            buf_to_flush.extend(buf);
        } else {
            self.current_buf.extend(buf);
        }
        if self.flushing {
            self.bufs_to_be_flushed.push_back(buf_to_flush);
            return None;
        }
        self.flushing = true;
        Some(self.write_buf(buf_to_flush))
    }

    fn write_buf(&mut self, buf: Vec<u8>) -> Action {
        Action::Write(self.file.as_raw_fd(), buf, self.offset, TRANSCRIPT_FLUSH)
    }
}

impl Drop for Transcript {
    fn drop(&mut self) {
        // Flushing should be done at this point, or else it's never
        // going to happen.  This assert should probably be a nop when
        // working on the unit tests, as otherwise if a panic unwinds
        // the stack then this panic here gets called too and
        // everything becomes a nightmare...
        // assert!(!self.flushing);
        if self.flushing {
            println!("WARN: flushing still in progress in Transcript");
        }
        if !self.compress_file {
            return;
        }

        // Close the file before compressing it; in normal shutdown
        // the io-uring is already idle at this point, so either
        // everything has been flushed or it hasn't been flushing.
        std::mem::drop(&mut self.file);

        // Compress transcript now that the file is closed
        match Command::new("xz").arg(&self.path).output() {
            Ok(output) => {
                if output.status.success() {
                    println!("\r\nTranscript saved to: {}.xz", self.path);
                } else {
                    println!("\r\nxz failed: {:?}", output);
                }
            }
            Err(e) => {
                println!("\r\nxz failed to start: {}", e);
            }
        }
    }
}

fn get_transcript() -> Result<(File, String)> {
    let home_dir = env::var("HOME")
        .map_err(|e| Error::new(ErrorKind::Other, format!("$HOME not in enviroment: {}", e)))?;
    let date_string = chrono::Local::now().to_rfc3339_opts(chrono::SecondsFormat::Secs, false);
    let path = format!("{}/Documents/lima-logs/log-{}", home_dir, date_string);
    let transcript = File::create(&path)?;
    Ok((transcript, path))
}

#[cfg(test)]
mod tests {
    use transcript::*;

    #[test]
    fn test_transcript_no_writes() {
        let file = File::create("/dev/null").unwrap();
        let mut t = Transcript::use_existing_file(file, String::from("/dev/null")).unwrap();
        {
            let a = t.start_teardown();
            assert!(a.is_none());
        }
    }

    #[test]
    fn test_transcript_one_small_write() {
        let file = File::create("/dev/null").unwrap();
        let fd = file.as_raw_fd();
        let mut t = Transcript::use_existing_file(file, String::from("/dev/null")).unwrap();
        assert!(t.log(&vec![0; 4]).is_none());
        {
            let a = t.start_teardown();
            assert!(a.is_some());
            check_write(&a.unwrap(), fd, 4, 0, TRANSCRIPT_FLUSH);
            assert!(t
                .handle_buffer_ev(4, vec![0; 4], TRANSCRIPT_FLUSH)
                .unwrap()
                .is_empty());
        }
    }

    #[test]
    fn test_transcript_two_large_writes() {
        let file = File::create("/dev/null").unwrap();
        let fd = file.as_raw_fd();
        let mut t = Transcript::use_existing_file(file, String::from("/dev/null")).unwrap();
        assert!(t.log(&vec![0; 2049]).is_none());
        {
            let a = t.log(&vec![0; 2049]);
            assert!(a.is_some());
            check_write(&a.unwrap(), fd, 2049, 0, TRANSCRIPT_FLUSH);
            assert!(t
                .handle_buffer_ev(2049, vec![0; 2049], TRANSCRIPT_FLUSH)
                .unwrap()
                .is_empty());
        }
        {
            let a = t.start_teardown();
            assert!(a.is_some());
            check_write(&a.unwrap(), fd, 2049, 2049, TRANSCRIPT_FLUSH);
            assert!(t
                .handle_buffer_ev(2049, vec![0; 2049], TRANSCRIPT_FLUSH)
                .unwrap()
                .is_empty());
        }
    }

    #[test]
    fn test_transcript_four_large_writes() {
        let file = File::create("/dev/null").unwrap();
        let fd = file.as_raw_fd();
        let mut t = Transcript::use_existing_file(file, String::from("/dev/null")).unwrap();
        assert!(t.log(&vec![0; 2049]).is_none());
        {
            let a = t.log(&vec![0; 2049]);
            assert!(a.is_some());
            check_write(&a.unwrap(), fd, 2049, 0, TRANSCRIPT_FLUSH);
            assert!(t
                .handle_buffer_ev(2049, vec![0; 2049], TRANSCRIPT_FLUSH)
                .unwrap()
                .is_empty());
        }
        {
            let a = t.log(&vec![0; 2049]);
            assert!(a.is_some());
            check_write(&a.unwrap(), fd, 2049, 2049, TRANSCRIPT_FLUSH);
            assert!(t
                .handle_buffer_ev(2049, vec![0; 2049], TRANSCRIPT_FLUSH)
                .unwrap()
                .is_empty());
        }
        {
            let a = t.log(&vec![0; 2049]);
            assert!(a.is_some());
            check_write(&a.unwrap(), fd, 2049, 4098, TRANSCRIPT_FLUSH);
            assert!(t
                .handle_buffer_ev(2049, vec![0; 2049], TRANSCRIPT_FLUSH)
                .unwrap()
                .is_empty());
        }
        {
            let a = t.log(&vec![0; 2049]);
            assert!(a.is_some());
            check_write(&a.unwrap(), fd, 2049, 6147, TRANSCRIPT_FLUSH);
            assert!(t
                .handle_buffer_ev(2049, vec![0; 2049], TRANSCRIPT_FLUSH)
                .unwrap()
                .is_empty());
        }
        {
            let a1 = t.start_teardown();
            assert!(a1.is_some());
            check_write(&a1.unwrap(), fd, 2049, 8196, TRANSCRIPT_FLUSH);
            // short write
            let a2 = t.handle_buffer_ev(4, vec![0; 2049], TRANSCRIPT_FLUSH);
            check_write(&a2.unwrap()[0], fd, 2045, 8200, TRANSCRIPT_FLUSH);
            let a3 = t.handle_buffer_ev(2045, vec![0; 2045], TRANSCRIPT_FLUSH);
            assert!(a3.unwrap().is_empty());
        }
    }
}
