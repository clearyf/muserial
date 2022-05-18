use std::fs;
use std::fs::File;
use std::fs::OpenOptions;
use std::io;
use std::io::prelude::{Read, Write};
use std::io::stdin;
use std::os::unix::io::{AsRawFd, FromRawFd, IntoRawFd, RawFd};

use utility::create_error;

extern crate libc;
use libc::*;

pub struct UartTty {
    previous_uart_settings: libc::termios,
    previous_tty_settings: libc::termios,
    uart_dev: fs::File,
}

pub struct Uart<'a> {
    uart: &'a mut UartTty,
}

impl<'a> Write for Uart<'a> {
    fn write(&mut self, buf: &[u8]) -> Result<usize, io::Error> {
        self.uart.uart_dev.write(buf)
    }
    fn flush(&mut self) -> Result<(), io::Error> {
        Ok(())
    }
}

impl<'a> Read for Uart<'a> {
    fn read(&mut self, buf: &mut [u8]) -> Result<usize, io::Error> {
        self.uart.uart_dev.read(buf)
    }
}

pub struct Tty {}

impl Write for Tty {
    fn write(&mut self, buf: &[u8]) -> Result<usize, io::Error> {
        // If this song & dance isn't done then the output is line-buffered.
        let mut stdout = unsafe { File::from_raw_fd(STDIN_FILENO) };
        let res = stdout.write(buf);
        // otherwise std::fs::File closes the fd.
        stdout.into_raw_fd();
        res
    }
    fn flush(&mut self) -> Result<(), io::Error> {
        Ok(())
    }
}

impl Read for Tty {
    fn read(&mut self, buf: &mut [u8]) -> Result<usize, io::Error> {
        stdin().read(buf)
    }
}

impl UartTty {
    pub fn new(dev_name: &str) -> Result<UartTty, io::Error> {
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
            previous_uart_settings: uart_settings,
            previous_tty_settings: tty_settings,
            uart_dev: dev,
        })
    }

    pub fn uart(&mut self) -> Uart {
        Uart { uart: self }
    }

    pub fn tty(&mut self) -> Tty {
        Tty {}
    }

    pub fn uart_fd(&self) -> RawFd {
        self.uart_dev.as_raw_fd()
    }
}

impl Drop for UartTty {
    fn drop(&mut self) {
        if let Err(e) = set_tty_settings(STDIN_FILENO, &self.previous_tty_settings) {
            println!("Couldn't restore tty settings: {}", e);
        }
        if let Err(e) = set_tty_settings(self.uart_dev.as_raw_fd(), &self.previous_uart_settings) {
            println!("Couldn't restore uart settings: {}", e);
        }
    }
}

fn get_tty_settings(fd: RawFd) -> Result<libc::termios, io::Error> {
    let mut settings = new_termios();
    if unsafe { tcgetattr(fd, &mut settings) } == 0 {
        Ok(settings)
    } else {
        create_error("Could not get tty settings")
    }
}

fn set_tty_settings(fd: RawFd, settings: &libc::termios) -> Result<(), io::Error> {
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
