mod uart_tty;
mod utility;

use crate::uart_tty::*;
use crate::utility::create_error;

use argparse::{ArgumentParser, Store};
use chrono::Local;
use futures_lite::{AsyncReadExt, future};
use smol::{Async, block_on};
use std::fs::File;
use std::io::{BufWriter, Write, self, stdin, stdout};
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

fn mainloop(dev_name: &str, logfile_path: &str) -> Result<ExitReason, io::Error> {
    let (uart_read, mut uart_write) = match open_uart(dev_name) {
        Ok(x) => x,
        Err(e) => {
            eprintln!("Could not open uart: {}", e);
            return Err(e);
        }
    };
    let mut logfile = match File::create(logfile_path) {
        Ok(file) => {
            eprintln!("\r\nLogfile: {}", logfile_path);
            Some(BufWriter::new(file))
        }
        Err(e) => {
            eprintln!("\r\nCouldn't open logfile, {:?}; proceeding without...", e);
            None
        }
    };

    let mut stdout = stdout();
    let mut stdin = Async::new(stdin())?;
    let mut stdin_buf = vec![0; BUFFER_SIZE];
    let mut uart_read = Async::new(uart_read)?;
    let mut uart_read_buf = vec![0; BUFFER_SIZE];
    loop {
        let stdin_fut = async { WhichRead::Stdin(stdin.read(&mut stdin_buf).await) };
        let uart_fut = async { WhichRead::Uart(uart_read.read(&mut uart_read_buf).await) };
        match block_on(future::or(stdin_fut, uart_fut)) {
            WhichRead::Stdin(result) => {
                let read_size = result?;
                if read_size == 0 {
                    return Ok(ExitReason::Eof);
                }
                let bytes_read = &stdin_buf[..read_size];
                let control_o: u8 = 0x0f;
                if bytes_read.contains(&control_o) {
                    return Ok(ExitReason::UserRequest);
                }
                uart_write.write_all(bytes_read)?;
                uart_write.flush()?;
            }
            WhichRead::Uart(result) => {
                let read_size = result?;
                if read_size == 0 {
                    return Ok(ExitReason::Eof);
                }
                let bytes_read = &uart_read_buf[..read_size];
                stdout.write_all(bytes_read)?;
                stdout.flush()?;
                if let Some(logfile) = logfile.as_mut() {
                    logfile.write_all(bytes_read)?;
                }
            }
        }
    }
}

fn get_logfile_path() -> Result<String, io::Error> {
    let home_dir = match std::env::var("HOME") {
        Ok(dir) => dir,
        Err(_) => return create_error("$HOME not defined?!"),
    };
    let time = Local::now().format("%Y-%m-%d_%H:%M:%S").to_string();
    Ok(format!("{}/Documents/lima-logs/log-{}", home_dir, time))
}

fn main() {
    let mut dev_name = "/dev/ttyUSB0".to_string();
    {
        let mut ap = ArgumentParser::new();
        ap.set_description("Connect to a serial line.");
        ap.refer(&mut dev_name)
            .add_argument("tty-device", Store, "Tty device to connect to");
        ap.parse_args_or_exit();
    }
    println!("Opening uart: {}", dev_name);

    let logfile_path = match get_logfile_path() {
        Ok(x) => x,
        Err(e) => {
            eprintln!(
                "\r\nError: Could not get path for logfile: {:?}, exiting...",
                e
            );
            return;
        }
    };

    match mainloop(&dev_name, &logfile_path) {
        Ok(ExitReason::UserRequest) => println!("\r\nQuit on user request"),
        Ok(ExitReason::Eof) => println!("\r\nGot Eof from {}", dev_name),
        e => eprintln!("\r\nError: {:?}", e),
    }

    if std::fs::metadata(&logfile_path).map_or(false, |m| m.is_file()) {
        match Command::new("xz").arg(&logfile_path).status() {
            Ok(status) => {
                if !status.success() {
                    eprintln!("Got {} on running xz on {}", status, logfile_path);
                }
            }
            Err(e) => {
                eprintln!("Got an error {} trying to run xz on {}", e, logfile_path);
            }
        }
    }
}
