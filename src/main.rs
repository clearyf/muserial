use std::io::Result;

extern crate libc;
use libc::*;

extern crate mio;
use mio::{Events, Ready, Poll, PollOpt, Token};
use mio::unix::EventedFd;

extern crate argparse;
use argparse::{ArgumentParser, Store, StoreConst};

mod uart_tty;
use uart_tty::{Action, CRNLTranslation, LocalEcho, UartTty};

mod utility;

fn main() {
    let mut local_echo = LocalEcho::Off;
    let mut crnl = CRNLTranslation::Off;
    let mut dev_name = "/dev/ttyUSB0".to_string();
    {
        let mut ap = ArgumentParser::new();
        ap.set_description("Connect to a serial line.");
        ap.refer(&mut local_echo).add_option(
            &["--local-echo"],
            StoreConst(LocalEcho::On),
            "Local echo enabled",
        );
        ap.refer(&mut crnl).add_option(
            &["--crnl-translation"],
            StoreConst(CRNLTranslation::On),
            "CR/NL translation enabled",
        );
        ap.refer(&mut dev_name).add_argument(
            "tty-device",
            Store,
            "Tty device to connect to",
        );
        ap.parse_args_or_exit();
    }
    println!("Opening uart: {}", dev_name);

    match mainloop(&dev_name, &local_echo, &crnl) {
        Ok(()) => println!("\nExiting on request"),
        Err(why) => println!("\nError: {}", why),
    }
}

fn mainloop(dev_name: &str, local_echo: &LocalEcho, crnl: &CRNLTranslation) -> Result<()> {
    let mut uart = UartTty::new(dev_name, *local_echo, *crnl)?;

    let poll = Poll::new()?;
    // Each id is the next bit; this way a bitmask can be used to
    // determine which fds are active.
    let stdin_id = Token(1 << 0);
    let uart_id = Token(1 << 1);
    poll.register(
        &EventedFd(&STDIN_FILENO),
        stdin_id,
        Ready::readable(),
        PollOpt::level(),
    )?;
    poll.register(
        &EventedFd(&uart.uart_fd()),
        uart_id,
        Ready::readable(),
        PollOpt::level(),
    )?;
    let mut events = Events::with_capacity(2);
    loop {
        poll.poll(&mut events, None)?;

        for event in &events {
            if event.token() == stdin_id {
                let ret = uart.copy_tty_to_uart()?;
                match ret {
                    Action::AllOk => (),
                    Action::Quit => return Ok(()),
                }
            } else if event.token() == uart_id {
                uart.copy_uart_to_tty()?;
            }
        }
    }
}
