use std::fs::{File, OpenOptions};
use std::io::Result;
use std::os::unix::io::{AsRawFd, RawFd};

use crate::utility::*;

use libc::*;

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

impl AsRawFd for UartTty {
    fn as_raw_fd(&self) -> RawFd {
        self.uart_dev.as_raw_fd()
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
