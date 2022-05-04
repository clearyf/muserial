use std::fs;
use std::fs::File;
use std::fs::OpenOptions;
use std::io::prelude::{Read, Write};
use std::io::{stdin, Result};
use std::os::unix::io::{AsRawFd, FromRawFd, IntoRawFd, RawFd};

use utility::create_error;

extern crate libc;
use libc::*;

const BUFFER_SIZE: usize = 64;

pub struct UartTty {
    uart_settings: libc::termios,
    tty_settings: libc::termios,
    uart_dev: fs::File,
}

pub enum Action {
    AllOk(Vec<u8>),
    Quit,
}

impl UartTty {
    pub fn new(dev_name: &str) -> Result<UartTty> {
        let dev = OpenOptions::new().read(true).write(true).open(dev_name)?;
        let tty_settings = get_tty_settings(STDIN_FILENO)?;
        set_tty_settings(STDIN_FILENO, &update_tty_settings(&tty_settings))?;

        let uart_settings = get_tty_settings(dev.as_raw_fd())?;
        set_tty_settings(dev.as_raw_fd(), &update_uart_settings(&uart_settings))?;

        unsafe {
            fcntl(dev.as_raw_fd(), F_SETFL, O_NONBLOCK);
            fcntl(STDIN_FILENO, F_SETFL, O_NONBLOCK);
        }

        Ok(UartTty {
            uart_settings: uart_settings,
            tty_settings: tty_settings,
            uart_dev: dev,
        })
    }

    pub fn read_from_tty(&mut self) -> Result<Action> {
        let mut buf = vec![0; BUFFER_SIZE];
        let read_size = stdin().read(&mut buf)?;
        if read_size == 0 {
            return create_error("No more data to read, port probably disconnected");
        }
        buf.truncate(read_size);

        let control_o: u8 = 0x0f;
        if buf.contains(&control_o) {
            Ok(Action::Quit)
        } else {
            Ok(Action::AllOk(buf))
        }
    }

    pub fn read_from_uart(&mut self) -> Result<Vec<u8>> {
        let mut buf = vec![0; BUFFER_SIZE];
        let read_size = self.uart_dev.read(&mut buf)?;
        if read_size == 0 {
            return create_error("No more data to read, port probably disconnected");
        }
        buf.truncate(read_size);
        Ok(buf)
    }

    pub fn write_to_tty(&mut self, buf: &[u8]) -> Result<usize> {
        // If this song & dance isn't done then the output is line-buffered.
        let mut stdout = unsafe { File::from_raw_fd(STDIN_FILENO) };
        let res = stdout.write(&buf);
        // otherwise std::fs::File closes the fd.
        stdout.into_raw_fd();
        res
    }

    pub fn write_to_uart(&mut self, buf: &[u8]) -> Result<usize> {
        self.uart_dev.write(&buf)
    }

    pub fn uart_fd(&self) -> RawFd {
        self.uart_dev.as_raw_fd()
    }
}

impl Drop for UartTty {
    fn drop(&mut self) {
        match set_tty_settings(STDIN_FILENO, &self.tty_settings) {
            Err(e) => println!("Couldn't restore tty settings: {}", e),
            _ => (),
        };
        match set_tty_settings(self.uart_dev.as_raw_fd(), &self.uart_settings) {
            Err(e) => println!("Couldn't restore uart settings: {}", e),
            _ => (),
        };
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
