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

pub struct Transcript {
    path: String,
    file: BufWriter<File>,
}

impl Transcript {
    pub fn new() -> Result<Transcript> {
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

#[derive(Debug)]
enum TtyState {
    NotStarted,
    Reading,
    Processing,
    Writing,
    TearDown,
}

#[derive(Debug)]
enum UartState {
    NotStarted,
    Reading,
    Processing,
    Writing,
    TearDown,
}

pub struct UartTtySM {
    uart_fd: i32,
    uart_state: UartState,
    tty_state: TtyState,
    transcript: Option<Transcript>,
}

impl UartTtySM {
    pub fn new(uart_fd: i32, transcript: Option<Transcript>) -> UartTtySM {
        UartTtySM {
            uart_fd: uart_fd,
            tty_state: TtyState::NotStarted,
            uart_state: UartState::NotStarted,
            transcript: transcript,
        }
    }

    pub fn init_actions(&mut self) -> Vec<Action> {
        self.tty_state = match self.tty_state {
            TtyState::NotStarted => TtyState::Reading,
            _ => panic!("UartTtySM::init_actions called multiple times!"),
        };
        self.uart_state = match self.uart_state {
            UartState::NotStarted => UartState::Reading,
            _ => panic!("UartTtySM::init_actions called multiple times!"),
        };
        vec![
            Action::Read(STDIN_FILENO, vec![0; DEFAULT_READ_SIZE], TTY_READ),
            Action::Read(self.uart_fd, vec![0; DEFAULT_READ_SIZE], UART_READ),
        ]
    }

    pub fn handle_other_ev(&mut self, result: i32, user_data: u64) -> Result<Vec<Action>> {
        match user_data {
            TTY_READ_CANCEL | UART_READ_CANCEL | TTY_WRITE_CANCEL | UART_WRITE_CANCEL => {
                return Ok(vec![])
            }
            _ => (),
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
        self.tty_state = match &self.tty_state {
            TtyState::Reading => TtyState::Processing,
            TtyState::TearDown => return Ok(vec![]),
            e => panic!("tty_read_done in invalid state: {:?}", e),
        };
        if result < 0 {
            return create_error(&format!("Got error from tty read: {}", result));
        }
        let control_o: u8 = 0x0f;
        if buf.contains(&control_o) {
            return self.start_teardown();
        } else {
            self.tty_state = match &self.tty_state {
                TtyState::Processing => TtyState::Writing,
                e => panic!("tty_read_done in invalid state: {:?}", e),
            };
            Ok(vec![Action::Write(self.uart_fd, buf, UART_WRITE)])
        }
    }

    fn uart_write_done(&mut self, result: i32, mut buf: Vec<u8>) -> Result<Vec<Action>> {
        self.tty_state = match &self.tty_state {
            TtyState::Writing => TtyState::Processing,
            TtyState::TearDown => return Ok(vec![]),
            e => panic!("uart_write_done in invalid state: {:?}", e),
        };
        if result == 0 {
            println!("\r\nPort disconnected\r");
            return self.start_teardown();
        } else if result < 0 {
            return create_error(&format!("Got error from uart write: {}", result));
        } else if (result as usize) < buf.len() {
            self.tty_state = match &self.tty_state {
                TtyState::Processing => TtyState::Writing,
                e => panic!("uart_write_done in invalid state: {:?}", e),
            };
            let new_buf = buf.split_off(result as usize);
            return Ok(vec![Action::Write(self.uart_fd, new_buf, UART_WRITE)]);
        }
        self.tty_state = match &self.tty_state {
            TtyState::Processing => TtyState::Reading,
            e => panic!("uart_write_done in invalid state: {:?}", e),
        };
        buf.resize(DEFAULT_READ_SIZE, 0);
        Ok(vec![Action::Read(STDIN_FILENO, buf, TTY_READ)])
    }

    fn uart_read_done(&mut self, result: i32, buf: Vec<u8>) -> Result<Vec<Action>> {
        self.uart_state = match &self.uart_state {
            UartState::Reading => UartState::Processing,
            UartState::TearDown => return Ok(vec![]),
            e => panic!("uart_read_done in invalid state: {:?}", e),
        };
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
        self.uart_state = match &self.uart_state {
            UartState::Processing => UartState::Writing,
            e => panic!("uart_read_done in invalid state: {:?}", e),
        };
        Ok(vec![Action::Write(STDIN_FILENO, buf, TTY_WRITE)])
    }

    fn tty_write_done(&mut self, result: i32, mut buf: Vec<u8>) -> Result<Vec<Action>> {
        self.uart_state = match &self.uart_state {
            UartState::Writing => UartState::Processing,
            UartState::TearDown => return Ok(vec![]),
            e => panic!("tty_write_done in invalid state: {:?}", e),
        };
        if result == 0 {
            // Impossible; no tty available to write to as it's closed!
            return self.start_teardown();
        } else if result < 0 {
            return create_error(&format!("Got error from tty write: {}", result));
        } else if (result as usize) < buf.len() {
            self.uart_state = match &self.uart_state {
                UartState::Processing => UartState::Writing,
                e => panic!("uart_write_done in invalid state: {:?}", e),
            };
            let new_buf = buf.split_off(result as usize);
            return Ok(vec![Action::Write(STDIN_FILENO, new_buf, TTY_WRITE)]);
        }
        self.uart_state = match &self.uart_state {
            UartState::Processing => UartState::Reading,
            e => panic!("uart_write_done in invalid state: {:?}", e),
        };
        buf.resize(DEFAULT_READ_SIZE, 0);
        Ok(vec![Action::Read(self.uart_fd, buf, UART_READ)])
    }

    fn start_teardown(&mut self) -> Result<Vec<Action>> {
        let mut actions = Vec::new();
        self.tty_state = match &self.tty_state {
            TtyState::Processing => TtyState::TearDown,
            TtyState::Reading => {
                actions.push(Action::Cancel(TTY_READ, TTY_READ_CANCEL));
                TtyState::TearDown
            }
            TtyState::Writing => {
                actions.push(Action::Cancel(UART_WRITE, UART_WRITE_CANCEL));
                TtyState::TearDown
            }
            e => panic!("start_teardown in invalid state: {:?}", e),
        };
        self.uart_state = match &self.uart_state {
            UartState::Processing => UartState::TearDown,
            UartState::Reading => {
                actions.push(Action::Cancel(UART_READ, UART_READ_CANCEL));
                UartState::TearDown
            }
            UartState::Writing => {
                actions.push(Action::Cancel(TTY_WRITE, TTY_WRITE_CANCEL));
                UartState::TearDown
            }
            e => panic!("start_teardown in invalid state: {:?}", e),
        };
        Ok(actions)
    }
}

#[cfg(test)]
mod tests {
    use uart_tty::*;

    fn check_read(action: &Action, expected_fd: i32, buf_len: usize, expected_user_data: u64) {
        match &action {
            Action::Read(fd, buf, user_data) => {
                assert_eq!(*fd, expected_fd);
                assert_eq!(buf.len(), buf_len);
                assert_eq!(*user_data, expected_user_data);
            }
            e => panic!("{:?}", e),
        };
    }

    fn check_write(action: &Action, expected_fd: i32, buf_len: usize, expected_user_data: u64) {
        match &action {
            Action::Write(fd, buf, user_data) => {
                assert_eq!(*fd, expected_fd);
                assert_eq!(buf.len(), buf_len);
                assert_eq!(*user_data, expected_user_data);
            }
            e => panic!("{:?}", e),
        };
    }

    fn check_cancel(action: &Action, expected_cancel_data: u64, expected_user_data: u64) {
        match &action {
            Action::Cancel(cancel_data, user_data) => {
                assert_eq!(*cancel_data, expected_cancel_data);
                assert_eq!(*user_data, expected_user_data);
            }
            e => panic!("{:?}", e),
        };
    }

    #[test]
    fn test_uartttysm() {
        let mut sm = UartTtySM::new(42, None);

        // Check init actions
        let init_actions = sm.init_actions();
        assert_eq!(init_actions.len(), 2);
        check_read(&init_actions[0], STDIN_FILENO, DEFAULT_READ_SIZE, TTY_READ);
        check_read(&init_actions[1], 42, DEFAULT_READ_SIZE, UART_READ);

        // First tty read
        let tty_read_actions = sm.handle_buffer_ev(3, vec![97, 98, 99], TTY_READ).unwrap();
        assert_eq!(tty_read_actions.len(), 1);
        check_write(&tty_read_actions[0], 42, 3, UART_WRITE);

        // Write to uart was short
        let uart_short_write_actions = sm
            .handle_buffer_ev(1, vec![97, 98, 99], UART_WRITE)
            .unwrap();
        assert_eq!(uart_short_write_actions.len(), 1);
        check_write(&uart_short_write_actions[0], 42, 2, UART_WRITE);

        // Write to uart now ok
        let tty_ok_write_actions = sm.handle_buffer_ev(2, vec![98, 99], UART_WRITE).unwrap();
        assert_eq!(tty_ok_write_actions.len(), 1);
        check_read(
            &tty_ok_write_actions[0],
            STDIN_FILENO,
            DEFAULT_READ_SIZE,
            TTY_READ,
        );

        // Now try uart
        let uart_read_actions = sm.handle_buffer_ev(3, vec![97, 98, 99], UART_READ).unwrap();
        assert_eq!(uart_read_actions.len(), 1);
        check_write(&uart_read_actions[0], STDIN_FILENO, 3, TTY_WRITE);

        // Tty write was short
        let tty_short_write_actions = sm.handle_buffer_ev(1, vec![97, 98, 99], TTY_WRITE).unwrap();
        assert_eq!(tty_short_write_actions.len(), 1);
        check_write(&tty_short_write_actions[0], STDIN_FILENO, 2, TTY_WRITE);

        // Tty write now ok
        let tty_ok_write_actions = sm.handle_buffer_ev(2, vec![98, 99], TTY_WRITE).unwrap();
        assert_eq!(tty_ok_write_actions.len(), 1);
        check_read(&tty_ok_write_actions[0], 42, DEFAULT_READ_SIZE, UART_READ);

        // Tty read to quit
        let tty_read_actions = sm.handle_buffer_ev(1, vec![15], TTY_READ).unwrap();
        assert_eq!(tty_read_actions.len(), 1);
        check_cancel(&tty_read_actions[0], UART_READ, UART_READ_CANCEL);

        let uart_read_cancel_actions = sm.handle_other_ev(-1, UART_READ_CANCEL).unwrap();
        assert!(uart_read_cancel_actions.is_empty());
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
