use std::io::Result;

extern crate libc;
use libc::*;

extern crate epoll;

mod epoll_helpers;
use epoll_helpers::{Epoller, Timeout};

extern crate argparse;
use argparse::{ArgumentParser, Store, StoreConst};

mod uart_tty;
use uart_tty::{Action, CRNLTranslation, LocalEcho, UartTty};

mod utility;
use utility::create_error;

fn main() {
    let mut dev_name = "".to_string();
    let mut local_echo = LocalEcho::Off;
    let mut crnl = CRNLTranslation::Off;
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
        ap.refer(&mut dev_name).required().add_argument(
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

    let epoll = Epoller::new()?;
    // Each id is the next bit; this way a bitmask can be used to
    // determine which fds are active.
    let stdin_id = 1 << 0;
    let uart_id = 1 << 1;
    epoll.add(STDIN_FILENO, stdin_id)?;
    epoll.add_oneshot(uart.uart_fd(), uart_id)?;

    let mut timeout = Timeout::Infinite;
    loop {
        // If the uart was active on last read, it is not immediately
        // rearmed; instead a short wait is made only on the tty.
        let events = epoll.wait(timeout)?;

        // Timed out, set infinite timer
        if events.is_empty() {
            epoll.rearm_oneshot(uart.uart_fd(), uart_id)?;
            timeout = Timeout::Infinite;
            continue;
        }
        let active: u64 = events.iter().sum();
        let allowable_bits: u64 = stdin_id | uart_id;
        if active | allowable_bits != allowable_bits {
            return create_error("epoll wait returned unknown events");
        }
        if active & stdin_id == stdin_id {
            let ret = uart.copy_tty_to_uart()?;
            match ret {
                Action::AllOk => (),
                Action::Quit => return Ok(()),
            }
            // Put the uart back into action to improve interactivity.
            epoll.rearm_oneshot(uart.uart_fd(), uart_id)?;
            timeout = Timeout::Infinite;
        }
        if active & uart_id == uart_id {
            uart.copy_uart_to_tty()?;
            // unit is milliseconds.  @115200 each millisecond is
            // about 14-15 bytes, so a 1024 byte buffer should easily
            // be able to handle 50ms (~750 bytes per 50ms).  However,
            // the buffer is not that big, so making this timeout too
            // large slows down the output considerably.  The output
            // also becomes laggy and jerky if this is too big.  20ms
            // is 50Hz, which should be ok.
            timeout = Timeout::Milliseconds(20);
        }
    }
}
