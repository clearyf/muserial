use std::env;
use std::fs::File;
use std::fs::OpenOptions;
use std::io::prelude::Write;
use std::io::{BufWriter, Error, ErrorKind, Result};
use std::os::unix::io::{AsRawFd, RawFd};
use std::process::Command;

use utility::{create_error, Action};

use libc::*;

const DEFAULT_READ_SIZE: usize = 1024;

const TTY_READ: u64 = 1;
const UART_READ: u64 = 2;
const TTY_WRITE: u64 = 3;
const UART_WRITE: u64 = 4;

const TTY_READ_CANCEL: u64 = 5;
const UART_READ_CANCEL: u64 = 6;
const TTY_WRITE_CANCEL: u64 = 7;
const UART_WRITE_CANCEL: u64 = 8;

pub struct UartTty {
    uart_settings: libc::termios,
    tty_settings: libc::termios,
    uart_dev: File,
}

impl UartTty {
    pub fn new(dev_name: &str) -> Result<UartTty> {
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
        })
    }

    pub fn uart_fd(&self) -> RawFd {
        self.uart_dev.as_raw_fd()
    }
}

impl Drop for UartTty {
    fn drop(&mut self) {
        if let Err(e) = set_tty_settings(STDIN_FILENO, &self.tty_settings) {
            println!("\r\nCouldn't restore tty settings: {}", e);
        }
        if let Err(e) = set_tty_settings(self.uart_dev.as_raw_fd(), &self.uart_settings) {
            println!("\r\nCouldn't restore uart settings: {}", e);
        }
    }
}

// enum UartState {
//     Idle,
//     Reading,
//     Writing,
//     TearDown,
// }

// enum TtyState {
//     Idle,
//     Reading,
//     Writing,
//     TearDown,
// }

struct Transcript {
    path: String,
    file: BufWriter<File>,
}

impl Transcript {
    fn new() -> Result<Transcript> {
        match get_transcript() {
            Ok((file, path)) => {
                println!("\r\nOpened transcript: {}", path);
                Ok(Transcript {
                    path: path,
                    file: file,
                })
            }
            Err(e) => {
                println!("\r\nCouldn't open transcript: {}", e);
                Err(e)
            }
        }
    }

    fn log(&mut self, buf: &[u8]) -> Result<()> {
        self.file.write_all(buf)
    }
}

impl Drop for Transcript {
    fn drop(&mut self) {
        if let Err(e) = self.file.flush() {
            println!("\r\nError while flushing transcript: {}", e);
        }

        // Close the file before compressing it
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

pub struct UartTtySM {
    uart_fd: i32,
    // When quit is requested by the user set this; in progress
    // operations should be cancelled.
    tear_down_in_progress: bool,
    // uart_state: UartState,
    // tty_state: TtyState,
    transcript: Option<Transcript>,
}

impl UartTtySM {
    pub fn new(uart_fd: i32) -> UartTtySM {
        let transcript = Transcript::new().ok();
        UartTtySM {
            uart_fd: uart_fd,
            tear_down_in_progress: false,
            // uart_state: UartState::Idle,
            // tty_state: TtyState::Idle,
            transcript: transcript,
        }
    }

    pub fn init_actions(&self) -> Vec<Action> {
        vec![
            Action::Read(STDIN_FILENO, vec![0; DEFAULT_READ_SIZE], TTY_READ),
            Action::Read(self.uart_fd, vec![0; DEFAULT_READ_SIZE], UART_READ),
        ]
    }

    pub fn handle_other_ev(&mut self, result: i32, user_data: u64) -> Result<Vec<Action>> {
        if self.tear_down_in_progress {
            return Ok(vec![]);
        }
        if result != 1 {
            return create_error(&format!(
                "Got unexpected result in handle_other_ev: {}",
                result
            ));
        }
        create_error(&format!(
            "Got unknown user_data in handle_other_ev: {}",
            user_data
        ))
    }

    pub fn handle_buffer_ev(
        &mut self,
        result: i32,
        buf: Vec<u8>,
        user_data: u64,
    ) -> Result<Vec<Action>> {
        if self.tear_down_in_progress {
            return Ok(vec![]);
        }
        if user_data == UART_READ {
            self.uart_read_done(result, buf)
        } else if user_data == TTY_READ {
            self.tty_read_done(result, buf)
        } else if user_data == UART_WRITE {
            self.uart_write_done(result, buf)
        } else if user_data == TTY_WRITE {
            self.tty_write_done(result, buf)
        } else {
            create_error(&format!(
                "Got unknown user_data in handle_buffer_ev: {}",
                user_data
            ))
        }
    }

    fn tty_read_done(&mut self, result: i32, buf: Vec<u8>) -> Result<Vec<Action>> {
        if result < 0 {
            return create_error(&format!("Got error from tty read: {}", result));
        }
        let control_o: u8 = 0x0f;
        if buf.contains(&control_o) {
            return self.start_teardown();
        } else {
            Ok(vec![Action::Write(self.uart_fd, buf, UART_WRITE)])
        }
    }

    fn uart_read_done(&mut self, result: i32, buf: Vec<u8>) -> Result<Vec<Action>> {
        if result == 0 {
            println!("\r\nPort disconnected\r");
            return self.start_teardown();
        } else if result < 0 {
            return create_error(&format!("Got error from uart read: {}", result));
        }
        // This is wrapped in a large bufwriter, so writes to the
        // transcript should be every few seconds at most; such writes
        // should also be extremely fast on any kind of remotely
        // modern hw.
        if let Some(transcript) = &mut self.transcript {
            transcript.log(&buf)?;
        }
        Ok(vec![Action::Write(STDIN_FILENO, buf, TTY_WRITE)])
    }

    fn uart_write_done(&mut self, _result: i32, mut buf: Vec<u8>) -> Result<Vec<Action>> {
        // Ignore write errors
        buf.resize(DEFAULT_READ_SIZE, 0);
        Ok(vec![Action::Read(STDIN_FILENO, buf, TTY_READ)])
    }

    fn tty_write_done(&mut self, _result: i32, mut buf: Vec<u8>) -> Result<Vec<Action>> {
        // Ignore write errors
        buf.resize(DEFAULT_READ_SIZE, 0);
        Ok(vec![Action::Read(self.uart_fd, buf, UART_READ)])
    }

    fn start_teardown(&mut self) -> Result<Vec<Action>> {
        self.tear_down_in_progress = true;
        // TODO Shouldn't need to send cancel for operations which are not in progress!
        Ok(vec![
            Action::Cancel(TTY_READ, TTY_READ_CANCEL),
            Action::Cancel(UART_READ, UART_READ_CANCEL),
            Action::Cancel(TTY_WRITE, TTY_WRITE_CANCEL),
            Action::Cancel(UART_WRITE, UART_WRITE_CANCEL),
        ])
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

fn get_transcript() -> Result<(BufWriter<File>, String)> {
    let home_dir = env::var("HOME")
        .map_err(|e| Error::new(ErrorKind::Other, format!("$HOME not in enviroment: {}", e)))?;
    let date_string = chrono::Local::now().to_rfc3339_opts(chrono::SecondsFormat::Secs, false);
    let path = format!("{}/Documents/lima-logs/log-{}", home_dir, date_string);
    let transcript = File::create(&path)?;
    // Default is 8kb, "but may change"
    Ok((BufWriter::with_capacity(64 * 1024, transcript), path))
}
