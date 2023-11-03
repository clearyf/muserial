use crate::utility::create_error;
use chrono::Local;
use std::fs::File;
use std::io::{BufWriter, Result, Write};
use std::process::Command;

pub struct Logfile {
    handle: Option<BufWriter<File>>,
    path: String,
}

impl Logfile {
    pub fn new() -> Result<Logfile> {
        let home_dir = match std::env::var("HOME") {
            Ok(dir) => dir,
            Err(_) => return create_error("$HOME not defined?!"),
        };
        let time = Local::now().format("%Y-%m-%d_%H:%M:%S").to_string();
        let path = format!("{}/Documents/lima-logs/log-{}", home_dir, time);
        Ok(Logfile {
            handle: Some(BufWriter::new(File::create(&path)?)),
            path: path,
        })
    }

    pub fn path(&self) -> &str {
        &self.path
    }

    pub fn log(&mut self, buf: &[u8]) -> Result<()> {
        if let Some(h) = &mut self.handle {
            h.write_all(buf)?;
        }
        Ok(())
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
