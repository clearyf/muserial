use crate::utility::create_error;

use libc::*;
use std::fs::{File, OpenOptions};
use std::io::{stdin, Read, Result, Stdin, Write};
use std::os::fd::{AsFd, AsRawFd, BorrowedFd};
use std::rc::Rc;
use async_io::IoSafe;

struct TermiosSetting<T: AsFd> {
    file: T,
    original_settings: libc::termios,
}

impl<T: AsFd> TermiosSetting<T> {
    fn from_stdin(file: Stdin) -> Result<TermiosSetting<Stdin>> {
        let original_settings = get_term_settings(file.as_fd())?;
        set_term_settings(file.as_fd(), &update_tty_settings(&original_settings))?;
        Ok(TermiosSetting {
            file,
            original_settings,
        })
    }
    fn from_uart(file: File) -> Result<TermiosSetting<File>> {
        let original_settings = get_term_settings(file.as_fd())?;
        set_term_settings(file.as_fd(), &update_uart_settings(&original_settings))?;
        Ok(TermiosSetting {
            file,
            original_settings,
        })
    }
}

impl<T: AsFd> Drop for TermiosSetting<T> {
    fn drop(&mut self) {
        if let Err(e) = set_term_settings(self.file.as_fd(), &self.original_settings) {
            println!("Couldn't restore tty settings: {}", e);
        }
    }
}

struct UartSettings {
    _previous_uart_settings: TermiosSetting<File>,
    _previous_tty_settings: TermiosSetting<Stdin>,
    file: File,
}

impl UartSettings {
    fn new(file: File) -> Result<UartSettings> {
        Ok(UartSettings {
            _previous_uart_settings: TermiosSetting::<File>::from_uart(file.try_clone()?)?,
            _previous_tty_settings: TermiosSetting::<Stdin>::from_stdin(stdin())?,
            file,
        })
    }
}

impl AsFd for UartSettings {
    fn as_fd(&self) -> BorrowedFd<'_> {
        self.file.as_fd()
    }
}

pub struct UartRead {
    inner: File,
    _settings: Rc<UartSettings>,
}

pub struct UartWrite {
    inner: File,
    _settings: Rc<UartSettings>,
}

pub fn open_uart(dev_name: &str) -> Result<(UartRead, UartWrite)> {
    let inner = OpenOptions::new().read(true).write(true).open(dev_name)?;
    let settings = Rc::new(UartSettings::new(inner.try_clone()?)?);
    let uart_read = UartRead {
        inner: inner.try_clone()?,
        _settings: settings.clone(),
    };
    let uart_write = UartWrite {
        inner,
        _settings: settings,
    };
    Ok((uart_read, uart_write))
}

impl AsFd for UartRead {
    fn as_fd(&self) -> BorrowedFd<'_> {
        self.inner.as_fd()
    }
}

impl Read for UartRead {
    fn read(&mut self, buf: &mut [u8]) -> Result<usize> {
        self.inner.read(buf)
    }
}

unsafe impl IoSafe for UartRead {}

impl Write for UartWrite {
    fn write(&mut self, buf: &[u8]) -> Result<usize> {
        self.inner.write(buf)
    }
    fn flush(&mut self) -> Result<()> {
        self.inner.flush()
    }
}

fn get_term_settings(fd: BorrowedFd) -> Result<libc::termios> {
    let mut settings = new_termios();
    if unsafe { tcgetattr(fd.as_raw_fd(), &mut settings) } == 0 {
        Ok(settings)
    } else {
        create_error("Could not get tty settings")
    }
}

fn set_term_settings(fd: BorrowedFd, settings: &libc::termios) -> Result<()> {
    if unsafe { tcflush(fd.as_raw_fd(), TCIFLUSH) } != 0 {
        return create_error("Could not flush tty device");
    }
    if unsafe { tcsetattr(fd.as_raw_fd(), TCSANOW, settings) } == 0 {
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
