use std::fs;
use std::fs::File;
use std::fs::OpenOptions;
use std::io::{stdin, Result};
use std::os::unix::io::{RawFd, AsRawFd, IntoRawFd, FromRawFd};
use std::io::prelude::{Read, Write};

use utility::create_error;

extern crate libc;
use libc::*;

pub struct UartTty {
    uart_settings: libc::termios,
    tty_settings: libc::termios,
    crnl_tranlation: CRNLTranslation,
    local_echo: LocalEcho,
    uart_dev: fs::File,
}

pub enum Action {
    AllOk,
    Quit,
}

#[derive(Clone, Copy)]
pub enum LocalEcho {
    On,
    Off,
}

#[derive(Clone, Copy)]
pub enum CRNLTranslation {
    On,
    Off,
}

impl UartTty {
    pub fn new(dev_name: &str, local_echo: LocalEcho, crnl: CRNLTranslation) -> Result<UartTty> {
        let dev = OpenOptions::new().read(true).write(true).open(dev_name)?;
        let tty_settings = get_tty_settings(STDIN_FILENO)?;
        set_tty_settings(STDIN_FILENO, &update_tty_settings(&tty_settings))?;

        let uart_settings = get_tty_settings(dev.as_raw_fd())?;
        // TODO allow changing of speed
        set_tty_settings(dev.as_raw_fd(), &update_uart_settings(&uart_settings))?;
        Ok(UartTty {
            uart_settings: uart_settings,
            tty_settings: tty_settings,
            crnl_tranlation: crnl,
            local_echo: local_echo,
            uart_dev: dev,
        })
    }

    pub fn copy_tty_to_uart(&mut self) -> Result<Action> {
        let mut buf = vec![0; 512];
        {
            let read_size = stdin().read(&mut buf)?;
            if read_size == 0 {
                return create_error("No more data to read, port probably disconnected");
            }
            buf.resize(read_size, 0);
        }
        let control_o: u8 = 0x0f;
        if buf.contains(&control_o) {
            Ok(Action::Quit)
        } else {
            self.uart_dev.write_all(&buf)?;
            // Echo back output, but convert carriage returns
            match self.local_echo {
                LocalEcho::On => {
                    buf = convert_char_to_crnl('\r', &buf);
                    write_to_tty(&buf)?;
                }
                LocalEcho::Off => (),
            };
            Ok(Action::AllOk)
        }
    }

    pub fn copy_uart_to_tty(&mut self) -> Result<Action> {
        let mut buf = vec![0; 512];
        {
            let read_size = self.uart_dev.read(&mut buf)?;
            if read_size == 0 {
                return create_error("No more data to read, port probably disconnected");
            }
            buf.resize(read_size, 0);
        }
        buf = match self.crnl_tranlation {
            CRNLTranslation::On => convert_char_to_crnl('\n', &buf),
            CRNLTranslation::Off => buf,
        };
        write_to_tty(&buf)?;
        Ok(Action::AllOk)
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

fn convert_char_to_crnl(ch: char, buf: &Vec<u8>) -> Vec<u8> {
    buf.iter().fold(
        Vec::new(),
        |mut vec, &c| if c == (ch as u8) {
            vec.push('\n' as u8);
            vec.push('\r' as u8);
            vec
        } else {
            vec.push(c);
            vec
        },
    )
}

fn write_to_tty(buf: &[u8]) -> Result<()> {
    // If this song & dance isn't done then the output is line-buffered.
    let mut stdout = unsafe { File::from_raw_fd(STDIN_FILENO) };
    stdout.write_all(&buf)?;
    // otherwise std::fs::File closes the fd.
    stdout.into_raw_fd();
    Ok(())
}
