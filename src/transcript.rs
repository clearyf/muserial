use std::cell::Cell;
use std::cell::RefCell;
use std::collections::VecDeque;
use std::env;
use std::fs::File;
use std::io::{Error, ErrorKind, Result};
use std::os::unix::io::AsRawFd;
use std::process::Command;
use std::rc::Rc;

use crate::reactor::*;

const TRANSCRIPT_BUFFER_SIZE: usize = 4096;

pub struct Transcript {
    path: String,
    file: File,
    offset: Cell<usize>,
    current_buf: RefCell<Vec<u8>>,
    flushing: Cell<bool>,
    bufs_to_be_flushed: RefCell<VecDeque<Vec<u8>>>,
    compress_file: bool,
}

impl Transcript {
    // #[cfg(test)]
    // fn use_existing_file(file: File, path: String) -> Result<Transcript> {
    //     Ok(Transcript {
    //         file: file,
    //         path: path,
    //         offset: Cell::new(0),
    //         current_buf: RefCell::new(Vec::with_capacity(TRANSCRIPT_BUFFER_SIZE)),
    //         flushing: Cell::new(false),
    //         bufs_to_be_flushed: RefCell::new(VecDeque::new()),
    //         compress_file: false,
    //     })
    // }

    pub fn new() -> Result<Transcript> {
        match get_transcript() {
            Ok((file, path)) => {
                println!("\r\nOpened transcript: {}\r", path);
                Ok(Transcript {
                    path: path,
                    file: file,
                    offset: Cell::new(0),
                    current_buf: RefCell::new(Vec::with_capacity(TRANSCRIPT_BUFFER_SIZE)),
                    flushing: Cell::new(false),
                    bufs_to_be_flushed: RefCell::new(VecDeque::new()),
                    compress_file: true,
                })
            }
            Err(e) => {
                println!("\r\nCouldn't open transcript: {}\r", e);
                Err(e)
            }
        }
    }
}

fn handle_buffer_ev(
    reactor: &mut dyn ReactorSubmitter,
    transcript: Rc<Transcript>,
    result: i32,
    mut buf: Vec<u8>,
    _user_data: u64,
) {
    assert!(transcript.flushing.get());
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
    transcript
        .offset
        .set(transcript.offset.get() + result as usize);
    if (result as usize) < buf.len() {
        // short write
        let new_buf = buf.split_off(result as usize);
        write_buf(reactor, transcript, new_buf);
        return;
    }
    // Queued buffer done, check for next
    let maybe_next_buf = transcript.bufs_to_be_flushed.borrow_mut().pop_front();
    if let Some(next_buf) = maybe_next_buf {
        write_buf(reactor, transcript, next_buf);
        return;
    }
    // Nothing more to do for the moment
    transcript.flushing.set(false);
}

pub fn start_transcript_teardown(reactor: &mut dyn ReactorSubmitter, transcript: Rc<Transcript>) {
    if transcript.flushing.get() {
        // Nothing to do right now, will continue flushing in the
        // background
        return;
    }
    assert!(transcript.bufs_to_be_flushed.borrow().is_empty());
    if transcript.current_buf.borrow().is_empty() {
        // Nothing more to do
        return;
    }
    transcript.flushing.set(true);
    let mut new_buf = Vec::with_capacity(TRANSCRIPT_BUFFER_SIZE);
    std::mem::swap(&mut new_buf, &mut transcript.current_buf.borrow_mut());
    write_buf(reactor, transcript, new_buf)
}

pub fn log_to_transcript(
    reactor: &mut dyn ReactorSubmitter,
    transcript: &Rc<Transcript>,
    buf: &[u8],
) {
    // Calculate new size of the buffer; if it would be larger
    // than the reserved size then copy the current buffer to a
    // new vec, and start flushing the current buffer.  I'm hoping
    // that normally the buffers being logged are <<< than the
    // TRANSCRIPT_BUFFER_SIZE, so most buffers flushed should be
    // fairly close to TRANSCRIPT_BUFFER_SIZE.
    let new_buf_size = buf.len() + transcript.current_buf.borrow().len();
    if new_buf_size < TRANSCRIPT_BUFFER_SIZE {
        transcript.current_buf.borrow_mut().extend(buf);
        return;
    }
    let mut buf_to_flush = Vec::with_capacity(TRANSCRIPT_BUFFER_SIZE);
    std::mem::swap(&mut *transcript.current_buf.borrow_mut(), &mut buf_to_flush);
    if new_buf_size == TRANSCRIPT_BUFFER_SIZE {
        buf_to_flush.extend(buf);
    } else {
        transcript.current_buf.borrow_mut().extend(buf);
    }
    if transcript.flushing.get() {
        transcript
            .bufs_to_be_flushed
            .borrow_mut()
            .push_back(buf_to_flush);
        return;
    }
    transcript.flushing.set(true);
    write_buf(reactor, transcript.clone(), buf_to_flush)
}

fn write_buf(reactor: &mut dyn ReactorSubmitter, transcript: Rc<Transcript>, buf: Vec<u8>) {
    reactor.submit_write(
        transcript.file.as_raw_fd(),
        buf,
        transcript.offset.get(),
        Box::new(move |reactor, result, buf, user_data| {
            handle_buffer_ev(reactor, transcript, result, buf, user_data)
        }),
    );
}

impl Drop for Transcript {
    fn drop(&mut self) {
        // Flushing should be done at this point, or else it's never
        // going to happen.  This assert should probably be a nop when
        // working on the unit tests, as otherwise if a panic unwinds
        // the stack then this panic here gets called too and
        // everything becomes a nightmare...
        // assert!(!self.flushing);
        if self.flushing.get() {
            println!("WARN: flushing still in progress in Transcript:drop!");
        }
        if self.current_buf.borrow().len() > 0 {
            println!("WARN: current buffer is non-empty in Transcript::drop!");
        }
        if self.bufs_to_be_flushed.borrow().len() > 0 {
            println!("WARN: unflushed buffers still present in Transcript::drop!");
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

// #[cfg(test)]
// mod tests {
//     use crate::transcript::*;

//     #[test]
//     fn test_transcript_no_writes() {
//         let file = File::create("/dev/null").unwrap();
//         let mut t = Transcript::use_existing_file(file, String::from("/dev/null")).unwrap();
//         {
//             let a = t.start_teardown();
//             assert!(a.is_none());
//         }
//     }

//     #[test]
//     fn test_transcript_one_small_write() {
//         let file = File::create("/dev/null").unwrap();
//         let fd = file.as_raw_fd();
//         let mut t = Transcript::use_existing_file(file, String::from("/dev/null")).unwrap();
//         assert!(t.log(&vec![0; 4]).is_none());
//         {
//             let a = t.start_teardown();
//             assert!(a.is_some());
//             check_write(&a.unwrap(), fd, 4, 0, TRANSCRIPT_FLUSH);
//             assert!(t
//                 .handle_buffer_ev(4, vec![0; 4], TRANSCRIPT_FLUSH)
//                 .unwrap()
//                 .is_empty());
//         }
//     }

//     #[test]
//     fn test_transcript_two_large_writes() {
//         let file = File::create("/dev/null").unwrap();
//         let fd = file.as_raw_fd();
//         let mut t = Transcript::use_existing_file(file, String::from("/dev/null")).unwrap();
//         assert!(t.log(&vec![0; 2049]).is_none());
//         {
//             let a = t.log(&vec![0; 2049]);
//             assert!(a.is_some());
//             check_write(&a.unwrap(), fd, 2049, 0, TRANSCRIPT_FLUSH);
//             assert!(t
//                 .handle_buffer_ev(2049, vec![0; 2049], TRANSCRIPT_FLUSH)
//                 .unwrap()
//                 .is_empty());
//         }
//         {
//             let a = t.start_teardown();
//             assert!(a.is_some());
//             check_write(&a.unwrap(), fd, 2049, 2049, TRANSCRIPT_FLUSH);
//             assert!(t
//                 .handle_buffer_ev(2049, vec![0; 2049], TRANSCRIPT_FLUSH)
//                 .unwrap()
//                 .is_empty());
//         }
//     }

//     #[test]
//     fn test_transcript_four_large_writes() {
//         let file = File::create("/dev/null").unwrap();
//         let fd = file.as_raw_fd();
//         let mut t = Transcript::use_existing_file(file, String::from("/dev/null")).unwrap();
//         assert!(t.log(&vec![0; 2049]).is_none());
//         {
//             let a = t.log(&vec![0; 2049]);
//             assert!(a.is_some());
//             check_write(&a.unwrap(), fd, 2049, 0, TRANSCRIPT_FLUSH);
//             assert!(t
//                 .handle_buffer_ev(2049, vec![0; 2049], TRANSCRIPT_FLUSH)
//                 .unwrap()
//                 .is_empty());
//         }
//         {
//             let a = t.log(&vec![0; 2049]);
//             assert!(a.is_some());
//             check_write(&a.unwrap(), fd, 2049, 2049, TRANSCRIPT_FLUSH);
//             assert!(t
//                 .handle_buffer_ev(2049, vec![0; 2049], TRANSCRIPT_FLUSH)
//                 .unwrap()
//                 .is_empty());
//         }
//         {
//             let a = t.log(&vec![0; 2049]);
//             assert!(a.is_some());
//             check_write(&a.unwrap(), fd, 2049, 4098, TRANSCRIPT_FLUSH);
//             assert!(t
//                 .handle_buffer_ev(2049, vec![0; 2049], TRANSCRIPT_FLUSH)
//                 .unwrap()
//                 .is_empty());
//         }
//         {
//             let a = t.log(&vec![0; 2049]);
//             assert!(a.is_some());
//             check_write(&a.unwrap(), fd, 2049, 6147, TRANSCRIPT_FLUSH);
//             assert!(t
//                 .handle_buffer_ev(2049, vec![0; 2049], TRANSCRIPT_FLUSH)
//                 .unwrap()
//                 .is_empty());
//         }
//         {
//             let a1 = t.start_teardown();
//             assert!(a1.is_some());
//             check_write(&a1.unwrap(), fd, 2049, 8196, TRANSCRIPT_FLUSH);
//             // short write
//             let a2 = t.handle_buffer_ev(4, vec![0; 2049], TRANSCRIPT_FLUSH);
//             check_write(&a2.unwrap()[0], fd, 2045, 8200, TRANSCRIPT_FLUSH);
//             let a3 = t.handle_buffer_ev(2045, vec![0; 2045], TRANSCRIPT_FLUSH);
//             assert!(a3.unwrap().is_empty());
//         }
//     }
// }