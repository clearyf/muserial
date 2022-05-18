use std::fs::File;
use std::io;
use std::io::{BufWriter, Write};
use std::process::Command;

extern crate chrono;
use chrono::Local;

pub struct Logfile {
    handle: Option<BufWriter<File>>,
    path: String,
}

impl Logfile {
    pub fn new() -> Result<Logfile, io::Error> {
        let p = Local::now()
            .format("/home/fionn/Documents/lima-logs/log-%Y-%m-%d_%H:%M:%S")
            .to_string();
        Ok(Logfile {
            handle: Some(BufWriter::new(File::create(&p)?)),
            path: p,
        })
    }

    pub fn log(&mut self, buf: &[u8]) -> Result<(), io::Error> {
        self.handle.as_mut().map(|h| h.write_all(buf)).unwrap()
    }
}

impl Drop for Logfile {
    fn drop(&mut self) {
        self.handle = None;
        match Command::new("xz").arg(&self.path).status() {
            Ok(status) => {
                if !status.success() {
                    eprintln!("Got {} on running xz on {}", status, self.path);
                }
            }
            Err(e) => {
                eprintln!("Got an error {} trying to run xz on {}", e, self.path);
            }
        }
    }
}
