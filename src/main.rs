use std::io::Result;

extern crate libc;
use libc::*;

extern crate mio;
use mio::{Events, Interest, Poll, Token};
use mio::unix::SourceFd;

extern crate argparse;
use argparse::{ArgumentParser, Store};

mod uart_tty;
use uart_tty::{Action, UartTty};

mod utility;

fn main() {
    let mut dev_name = "/dev/ttyUSB0".to_string();
    {
        let mut ap = ArgumentParser::new();
        ap.set_description("Connect to a serial line.");
        ap.refer(&mut dev_name).add_argument(
            "tty-device",
            Store,
            "Tty device to connect to",
        );
        ap.parse_args_or_exit();
    }
    println!("Opening uart: {}", dev_name);

    match mainloop(&dev_name) {
        Ok(()) => println!("\nExiting on request"),
        Err(why) => println!("\nError: {}", why),
    }
}

const STDIN_ID: Token = Token(0);
const UART_ID: Token = Token(1);

fn mainloop(dev_name: &str) -> Result<()> {
    let mut uart = UartTty::new(dev_name)?;
    let mut poll = Poll::new()?;

    poll.registry().register(
        &mut SourceFd(&STDIN_FILENO),
        STDIN_ID,
        Interest::READABLE,
    )?;
    poll.registry().register(
        &mut SourceFd(&uart.uart_fd()),
        UART_ID,
        Interest::READABLE,
    )?;
    let mut events = Events::with_capacity(2);
    loop {
        poll.poll(&mut events, None)?;

        for event in &events {
            if event.token() == STDIN_ID {
                let ret = uart.copy_tty_to_uart()?;
                match ret {
                    Action::AllOk => (),
                    Action::Quit => return Ok(()),
                }
            } else if event.token() == UART_ID {
                uart.copy_uart_to_tty()?;
            } else {
                panic!("Unknown id in poll!");
            }
        }
    }
}
