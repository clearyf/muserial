use std::collections::VecDeque;
use std::io;
use std::io::Write;

// Shrink the buffer queues to this size if they ever grow larger.
const BUF_QUEUE_SIZE: usize = 4;

// The size of the buffers in the queue.
const MAX_BUF_SIZE: usize = 4096;

pub struct BufQueue {
    queue: VecDeque<Vec<u8>>,
    written: usize,
}

impl BufQueue {
    pub fn new() -> BufQueue {
        BufQueue {
            queue: VecDeque::new(),
            written: 0,
        }
    }

    pub fn is_empty(&self) -> bool {
        self.queue.is_empty()
    }

    fn enqueue(&mut self, buf: &[u8]) {
        match self.queue.front_mut() {
            None => self.queue.push_back(buf.to_vec()),
            Some(front_buf) if front_buf.len() + buf.len() < MAX_BUF_SIZE => front_buf.extend(buf),
            Some(_) => self.queue.push_back(buf.to_vec()),
        };
    }

    pub fn try_write_or_enqueue<T: Write>(
        &mut self,
        buf: &[u8],
        mut writable: T,
    ) -> Result<(), io::Error> {
        if !self.queue.is_empty() {
            self.enqueue(buf);
            return Ok(());
        }

        match writable.write(buf) {
            Ok(written) if written == buf.len() => Ok(()),
            Ok(written) => {
                // Partial write
                self.enqueue(&buf[written..]);
                Ok(())
            }
            Err(e) if e.kind() == io::ErrorKind::WouldBlock => {
                // Try again later
                self.enqueue(buf);
                Ok(())
            }
            Err(e) => Err(e),
        }
    }

    pub fn flush<T: Write>(&mut self, mut writable: T) -> Result<(), io::Error> {
        loop {
            match self.queue.front() {
                None => {
                    self.queue.shrink_to(BUF_QUEUE_SIZE);
                    return Ok(());
                }
                Some(buf) => {
                    // Get slice to remaining bytes to write
                    let buf = buf.get(self.written..).unwrap();
                    match writable.write(buf) {
                        Ok(written) if written == buf.len() => {
                            self.queue.pop_front();
                            self.written = 0;
                        }
                        Ok(written) => {
                            self.written += written;
                            // partial write, no point in looping
                            // anymore as this should only happen
                            // if there wasn't enough room in the
                            // receving buffer.
                            return Ok(());
                        }
                        Err(e) if e.kind() == io::ErrorKind::WouldBlock => return Ok(()),
                        Err(e) => return Err(e),
                    };
                }
            }
        }
    }
}
