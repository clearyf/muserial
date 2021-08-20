use std::io::Result;
use std::os::unix::io::AsRawFd;

use argparse::{ArgumentParser, Store};

use libc::STDIN_FILENO;

mod reactor;
use crate::reactor::Reactor;

mod uart_tty;
use crate::uart_tty::UartTty;
mod uart_tty_sm;
use crate::uart_tty_sm::UartTtySM;

mod transcript;
use crate::transcript::Transcript;

mod utility;

fn main() {
    let mut dev_name = String::from("/dev/ttyUSB0");
    {
        let mut ap = ArgumentParser::new();
        ap.set_description("Connect to a serial line.");
        ap.refer(&mut dev_name)
            .add_argument("tty-device", Store, "Tty device to connect to");
        ap.parse_args_or_exit();
    }
    println!("Opening uart: {}", dev_name);

    match mainloop(dev_name) {
        Ok(()) => println!("\r"),
        Err(why) => println!("\nError: {}", why),
    }
}

struct LocalTty {}

impl LocalTty {
    fn new() -> LocalTty {
        LocalTty {}
    }
}

impl AsRawFd for LocalTty {
    fn as_raw_fd(&self) -> i32 {
        STDIN_FILENO
    }
}

fn mainloop(dev_name: String) -> Result<()> {
    let mut r = Reactor::new(4)?;
    UartTtySM::init_actions(
        &mut r,
        Box::new(LocalTty::new()),
        Box::new(UartTty::new(&dev_name)?),
        Some(Transcript::new()?),
    );
    r.run()
}
