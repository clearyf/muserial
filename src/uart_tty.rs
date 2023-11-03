use crate::utility::create_error;
use libc::*;
use std::fs::{File, OpenOptions};
use std::io::{Read, Result, Stdin, Write, stdin};
use std::os::unix::io::{AsRawFd, RawFd};
use std::rc::Rc;

struct TermiosSetting<T: AsRawFd> {
    file: T,
    original_settings: libc::termios,
}

impl<T: AsRawFd> TermiosSetting<T> {
    fn from_stdin(file: Stdin) -> Result<TermiosSetting<Stdin>> {
        let settings = get_term_settings(file.as_raw_fd())?;
        set_term_settings(file.as_raw_fd(), &update_tty_settings(&settings))?;
        Ok(TermiosSetting {
            file: file,
            original_settings: settings,
        })
    }
    fn from_uart(file: File) -> Result<TermiosSetting<File>> {
        let settings = get_term_settings(file.as_raw_fd())?;
        set_term_settings(file.as_raw_fd(), &update_uart_settings(&settings))?;
        Ok(TermiosSetting {
            file: file,
            original_settings: settings,
        })
    }
}

impl<T: AsRawFd> Drop for TermiosSetting<T> {
    fn drop(&mut self) {
        if let Err(e) = set_term_settings(self.file.as_raw_fd(), &self.original_settings) {
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
            file: file,
        })
    }
}

impl AsRawFd for UartSettings {
    fn as_raw_fd(&self) -> i32 {
        self.file.as_raw_fd()
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

fn get_term_settings(fd: RawFd) -> Result<libc::termios> {
    let mut settings = new_termios();
    if unsafe { tcgetattr(fd, &mut settings) } == 0 {
        Ok(settings)
    } else {
        create_error("Could not get tty settings")
    }
}

fn set_term_settings(fd: RawFd, settings: &libc::termios) -> Result<()> {
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
