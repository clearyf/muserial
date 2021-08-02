use std::env;
use std::fs::File;
use std::fs::OpenOptions;
use std::io::prelude::Write;
use std::io::{BufWriter, Error, ErrorKind, Result};
use std::os::unix::io::{AsRawFd, FromRawFd, IntoRawFd, RawFd};
use std::process::Command;

use utility::{create_error, Action};

use libc::*;

const DEFAULT_READ_SIZE: usize = 1024;
const STDIN_READ: u64 = 1;
const UART_READ: u64 = 2;
const UART_READ_CANCEL: u64 = 3;
const STDIN_READ_CANCEL: u64 = 3;

pub struct UartTty {
    uart_settings: libc::termios,
    tty_settings: libc::termios,
    uart_dev: File,
    logfile: Option<(BufWriter<File>, String)>,
    // When quit is requested by the user set this; in progress
    // operations should be cancelled.
    tear_down_in_progress: bool,
}

impl UartTty {
    pub fn new(dev_name: &str) -> Result<UartTty> {
        let logfile = match get_logfile() {
            Ok((logfile, path)) => {
                println!("Created new logfile: {}", &path);
                Some((logfile, path))
            }
            Err(e) => {
                println!("Couldn't open logfile: {}", e);
                None
            }
        };
        let dev = OpenOptions::new().read(true).write(true).open(dev_name)?;
        let tty_settings = get_tty_settings(STDIN_FILENO)?;
        set_tty_settings(STDIN_FILENO, &update_tty_settings(&tty_settings))?;

        let uart_settings = get_tty_settings(dev.as_raw_fd())?;
        // TODO allow changing of speed
        set_tty_settings(dev.as_raw_fd(), &update_uart_settings(&uart_settings))?;
        Ok(UartTty {
            uart_settings: uart_settings,
            tty_settings: tty_settings,
            uart_dev: dev,
            logfile: logfile,
            tear_down_in_progress: false,
        })
    }

    pub fn init_actions(&self) -> Vec<Action> {
        vec![
            Action::Read(STDIN_FILENO, vec![0; DEFAULT_READ_SIZE], STDIN_READ),
            Action::Read(self.uart_fd(), vec![0; DEFAULT_READ_SIZE], UART_READ),
        ]
    }

    pub fn handle_poll(&mut self, result: i32, user_data: u64) -> Result<Action> {
        if self.tear_down_in_progress {
            // do nothing
            return Ok(Action::NoOp);
        }
        if result != 1 {
            return create_error(&format!("Got unexpected result from poll: {}", result));
        }
        if user_data == UART_READ_CANCEL {
            self.tear_down_in_progress = true;
            Ok(Action::NoOp)
        }
        else {
            create_error(&format!("Got unknown user_data from poll: {}", user_data))
        }
    }

    pub fn handle_buffer(
        &mut self,
        result: i32,
        mut buf: Vec<u8>,
        user_data: u64,
    ) -> Result<Action> {
        if self.tear_down_in_progress {
            // do nothing
            return Ok(Action::NoOp);
        }
        if result <= 0 {
            // return create_error(&format!(
            //     "Got unexpected result in handle_buffer: {}",
            //     result
            // ));
            buf.clear();
        }
        else {
            buf.resize(result as usize, 0);
        }
        if user_data == UART_READ {
            self.copy_uart_to_tty(buf) }
        else if user_data == STDIN_READ {
            self.copy_tty_to_uart(buf)
        } else {
            create_error(&format!("Got unknown user_data from poll: {}", user_data))
        }
    }

    fn copy_tty_to_uart(&mut self, mut buf: Vec<u8>) -> Result<Action> {
        let control_o: u8 = 0x0f;
        if buf.contains(&control_o) {
            self.tear_down_in_progress = true;
            Ok(Action::Cancel(UART_READ, UART_READ_CANCEL))
        } else {
            self.uart_dev.write_all(&buf)?;
            buf.resize(DEFAULT_READ_SIZE, 0);
            Ok(Action::Read(STDIN_FILENO, buf, STDIN_READ))
        }
    }

    fn copy_uart_to_tty(&mut self, mut buf: Vec<u8>) -> Result<Action> {
        if buf.len() == 0 {
            self.tear_down_in_progress = true;
            return Ok(Action::Cancel(STDIN_READ, STDIN_READ_CANCEL))
        }
        write_to_tty(&buf)?;
        if let Some((logfile, _)) = &mut self.logfile {
            logfile.write_all(&buf)?;
        }
        buf.resize(DEFAULT_READ_SIZE, 0);
        Ok(Action::Read(self.uart_fd(), buf, UART_READ))
    }

    fn uart_fd(&self) -> RawFd {
        self.uart_dev.as_raw_fd()
    }
}

impl Drop for UartTty {
    fn drop(&mut self) {
        if let Err(e) = set_tty_settings(STDIN_FILENO, &self.tty_settings) {
            println!("Couldn't restore tty settings: {}", e);
        }
        if let Err(e) = set_tty_settings(self.uart_dev.as_raw_fd(), &self.uart_settings) {
            println!("Couldn't restore uart settings: {}", e);
        }
        if let Some((logfile, path)) = &mut self.logfile {
            if let Err(e) = logfile.flush() {
                println!("Error while flushing logfile: {}", e);
            }
            // Steal the path and then close the file
            let path = std::mem::take(path);
            self.logfile = None;

            // Compress logfile now that the file is closed
            match Command::new("xz").arg(path).output() {
                Ok(output) => {
                    if !output.status.success() {
                        println!("xz failed: {:?}", output);
                    }
                }
                Err(e) => {
                    println!("xz failed to start: {}", e);
                }
            }
        }
    }
}

fn get_tty_settings(fd: RawFd) -> Result<libc::termios> {
    let mut settings = new_termios();
    if unsafe { tcgetattr(fd, &mut settings) } == 0 {
        Ok(settings)
    } else {
        create_error("Could not get tty settings")
    }
}

fn set_tty_settings(fd: RawFd, settings: &libc::termios) -> Result<()> {
    if unsafe { tcflush(fd, TCIFLUSH) } != 0 {
        return create_error("Could not flush tty device");
    }
    if unsafe { tcsetattr(fd, TCSANOW, settings) } == 0 {
        Ok(())
    } else {
        create_error("Could not set tty settings")
    }
}

fn update_tty_settings(orig: &libc::termios) -> libc::termios {
    let mut settings = *orig;
    settings.c_iflag = IGNPAR;
    settings.c_oflag = 0;
    settings.c_cflag = B115200 | CRTSCTS | CS8 | CLOCAL | CREAD;
    settings.c_lflag = 0;
    settings.c_cc[VMIN] = 1;
    settings.c_cc[VTIME] = 0;
    settings
}

fn update_uart_settings(orig: &libc::termios) -> libc::termios {
    let mut settings = update_tty_settings(orig);
    settings.c_cflag = B115200 | CS8 | CLOCAL | CREAD;
    settings
}

fn new_termios() -> libc::termios {
    libc::termios {
        c_iflag: 0,
        c_oflag: 0,
        c_cflag: 0,
        c_lflag: 0,
        c_line: 0,
        c_cc: [0; 32],
        c_ispeed: 0,
        c_ospeed: 0,
    }
}

fn write_to_tty(buf: &[u8]) -> Result<()> {
    // If this song & dance isn't done then the output is line-buffered.
    let mut stdout = unsafe { File::from_raw_fd(STDIN_FILENO) };
    stdout.write_all(&buf)?;
    // otherwise std::fs::File closes the fd.
    stdout.into_raw_fd();
    Ok(())
}

fn get_logfile() -> Result<(BufWriter<File>, String)> {
    let home_dir = env::var("HOME")
        .map_err(|e| Error::new(ErrorKind::Other, format!("$HOME not in enviroment: {}", e)))?;
    let date_string = chrono::Local::now().to_rfc3339_opts(chrono::SecondsFormat::Secs, false);
    let path = format!("{}/Documents/lima-logs/log-{}", home_dir, date_string);
    let logfile = File::create(&path)?;
    Ok((BufWriter::new(logfile), path))
}
