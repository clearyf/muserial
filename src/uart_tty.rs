use crate::utility::*;
use libc::*;
use std::fs::{File, OpenOptions};
use std::io::{Read, Result, Write};
use std::os::unix::io::{AsRawFd, RawFd};
use std::rc::Rc;

struct UartSettings {
    previous_uart_settings: libc::termios,
    previous_tty_settings: libc::termios,
    file: File,
}

pub struct UartRead {
    inner: File,
    _settings: Rc<UartSettings>,
}

pub struct UartWrite {
    inner: File,
    _settings: Rc<UartSettings>,
}

pub fn create_uart(dev_name: &str) -> Result<(UartRead, UartWrite)> {
    let file = OpenOptions::new().read(true).write(true).open(dev_name)?;
    let inner = Rc::new(UartSettings::new(file.try_clone()?)?);
    let uart_read = UartRead {
        inner: file.try_clone()?,
        _settings: inner.clone(),
    };
    let uart_write = UartWrite {
        inner: file,
        _settings: inner,
    };
    Ok((uart_read, uart_write))
}

impl AsRawFd for UartRead {
    fn as_raw_fd(&self) -> i32 {
        self.inner.as_raw_fd()
    }
}

impl Read for UartRead {
    fn read(&mut self, buf: &mut [u8]) -> Result<usize> {
        self.inner.read(buf)
    }
}

impl AsRawFd for UartWrite {
    fn as_raw_fd(&self) -> i32 {
        self.inner.as_raw_fd()
    }
}

impl Write for UartWrite {
    fn write(&mut self, buf: &[u8]) -> Result<usize> {
        self.inner.write(buf)
    }
    fn flush(&mut self) -> Result<()> {
        self.inner.flush()
    }
}

impl UartSettings {
    fn new(file: File) -> Result<UartSettings> {
        let tty_settings = get_tty_settings(STDIN_FILENO)?;
        set_tty_settings(STDIN_FILENO, &update_tty_settings(&tty_settings))?;

        let uart_settings = get_tty_settings(file.as_raw_fd())?;
        set_tty_settings(file.as_raw_fd(), &update_uart_settings(&uart_settings))?;

        Ok(UartSettings {
            previous_uart_settings: uart_settings,
            previous_tty_settings: tty_settings,
            file: file,
        })
    }
}

impl Drop for UartSettings {
    fn drop(&mut self) {
        if let Err(e) = set_tty_settings(STDIN_FILENO, &self.previous_tty_settings) {
            println!("Couldn't restore tty settings: {}", e);
        }
        if let Err(e) = set_tty_settings(self.as_raw_fd(), &self.previous_uart_settings) {
            println!("Couldn't restore uart settings: {}", e);
        }
    }
}

impl AsRawFd for UartSettings {
    fn as_raw_fd(&self) -> i32 {
        self.file.as_raw_fd()
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
