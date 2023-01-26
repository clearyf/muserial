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
        let p = Local::now()
            .format("/home/fionn/Documents/lima-logs/log-%Y-%m-%d_%H:%M:%S")
            .to_string();
        Ok(Logfile {
            handle: Some(BufWriter::new(File::create(&p)?)),
            path: p,
        })
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
