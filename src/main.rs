mod uart_tty;

use crate::uart_tty::*;

use argparse::{ArgumentParser, Store};
use chrono::Local;
use futures_lite::{future, AsyncReadExt};
use smol::{block_on, Async};
use snafu::{prelude::*, Whatever};
use std::fs::File;
use std::io::{self, stdin, stdout, BufWriter, Write};
use std::process::Command;

const BUFFER_SIZE: usize = 128;

#[derive(Debug)]
enum ExitReason {
    Eof,
    UserRequest,
}

enum WhichRead {
    Uart(Result<usize, io::Error>),
    Stdin(Result<usize, io::Error>),
}

struct Logfile {
    file: Option<BufWriter<File>>,
    path: String,
}

fn get_logfile_path() -> Result<String, Whatever> {
    let home_dir = match std::env::var("HOME") {
        Ok(dir) => dir,
        Err(_) => whatever!("$HOME not defined?!"),
    };
    let time = Local::now().format("%Y-%m-%d_%H:%M:%S").to_string();
    Ok(format!("{}/Documents/lima-logs/log-{}", home_dir, time))
}

impl Logfile {
    fn new() -> Result<Logfile, Whatever> {
        let path = get_logfile_path()?;
        let logfile = File::create(&path)
            .with_whatever_context(|_| format!("Could not open logfile: {}", &path))?;
        Ok(Logfile {
            file: Some(BufWriter::new(logfile)),
            path,
        })
    }
}

impl io::Write for Logfile {
    fn write(&mut self, buf: &[u8]) -> Result<usize, io::Error> {
        if let Some(f) = &mut self.file {
            return f.write(buf);
        }
        panic!("Logfile struct did not contain a file handle in write!");
    }
    fn flush(&mut self) -> Result<(), io::Error> {
        if let Some(f) = &mut self.file {
            return f.flush();
        }
        panic!("Logfile struct did not contain a file handle in flush!");
    }
}

impl Drop for Logfile {
    fn drop(&mut self) {
        {
            let _ = std::mem::take(&mut self.file);
            // File is dropped here
        }

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

fn mainloop(dev_name: &str) -> Result<ExitReason, Whatever> {
    let (uart_read, mut uart_write) = open_uart(dev_name)?;
    println!("Opened UART: {}", dev_name);
    let mut logfile = Logfile::new()?;
    let mut stdout = stdout();
    let mut stdin = Async::new(stdin()).whatever_context("Could not create async stdin")?;
    let mut stdin_buf = vec![0; BUFFER_SIZE];
    let mut uart_read = Async::new(uart_read).whatever_context("Could not create async UART")?;
    let mut uart_read_buf = vec![0; BUFFER_SIZE];
    loop {
        let stdin_fut = async { WhichRead::Stdin(stdin.read(&mut stdin_buf).await) };
        let uart_fut = async { WhichRead::Uart(uart_read.read(&mut uart_read_buf).await) };
        match block_on(future::or(stdin_fut, uart_fut)) {
            WhichRead::Stdin(result) => {
                let read_size = result.whatever_context("Could not read from stdin")?;
                if read_size == 0 {
                    return Ok(ExitReason::Eof);
                }
                let bytes_read = &stdin_buf[..read_size];
                let control_o: u8 = 0x0f;
                if bytes_read.contains(&control_o) {
                    return Ok(ExitReason::UserRequest);
                }
                uart_write
                    .write_all(bytes_read)
                    .whatever_context("Could not write to UART")?;
                uart_write
                    .flush()
                    .whatever_context("Could not flush UART")?;
            }
            WhichRead::Uart(result) => {
                let read_size = result.whatever_context("Could not read from UART")?;
                if read_size == 0 {
                    return Ok(ExitReason::Eof);
                }
                let bytes_read = &uart_read_buf[..read_size];
                stdout
                    .write_all(bytes_read)
                    .whatever_context("Could not write to stdout")?;
                stdout.flush().whatever_context("Could not flush stdout")?;
                logfile
                    .write_all(bytes_read)
                    .whatever_context("Could not write all bytes to logfile")?;
            }
        }
    }
}

#[snafu::report]
fn main() -> Result<(), Whatever> {
    let mut dev_name = "/dev/ttyUSB0".to_string();
    {
        let mut ap = ArgumentParser::new();
        ap.set_description("Connect to a serial line.");
        ap.refer(&mut dev_name)
            .add_argument("tty-device", Store, "Tty device to connect to");
        ap.parse_args_or_exit();
    }
    match mainloop(&dev_name)? {
        ExitReason::UserRequest => println!("\r\nQuit on user request"),
        ExitReason::Eof => println!("\r\nGot Eof from {}", dev_name),
    };
    Ok(())
}
