mod logfile;
mod sendstop;
mod uart_tty;
mod utility;
use crate::logfile::*;
use crate::sendstop::*;
use crate::uart_tty::*;
use argparse::{ArgumentParser, Store};
use futures_concurrency::future::{FutureExt, Join};
use futures_lite::{AsyncReadExt, AsyncWriteExt};
use smol::Async;
use std::io::{stdin, stdout, Result};

const BUFFER_SIZE: usize = 128;

#[derive(Debug)]
enum Reason {
    EOF,
    UserRequest,
    GotStop,
}

async fn do_read<T>(stop: &SendStop, src: &mut T, mut buf: &mut [u8]) -> Option<Result<usize>>
where
    T: AsyncReadExt + Unpin,
{
    let stop_fut = async {
        stop.should_stop().await;
        None
    };
    let read_fut = async { Some(src.read(&mut buf).await) };
    stop_fut.race(read_fut).await
}

async fn uart_read_loop(stop: SendStop, uart: UartRead) -> Result<Reason> {
    let mut logfile = match Logfile::new() {
        Ok(file) => {
            eprintln!("\r\nLogfile: {}", file.path());
            Some(file)
        },
        Err(e) => {
            eprintln!("\r\nCouldn't open logfile, {:?}; proceeding without...", e);
            None
        }
    };
    let mut buf = vec![0; BUFFER_SIZE];
    let mut uart = Async::new(uart)?;
    let mut stdout = Async::new(stdout())?;

    while let Some(result) = do_read(&stop, &mut uart, &mut buf).await {
        let read_size = result?;
        if read_size == 0 {
            return Ok(Reason::EOF);
        }
        let bytes_read = &buf[..read_size];
        stdout.write_all(&bytes_read).await?;
        stdout.flush().await?;
        // No point doing anything non-blocking here, we're writing a
        // few kb to a file on a local filesystem; the OS will buffer
        // for us.
        if let Some(logfile) = logfile.as_mut() {
            logfile.log(&bytes_read)?;
        }
    }
    Ok(Reason::GotStop)
}

async fn uart_write_loop(stop: SendStop, uart: UartWrite) -> Result<Reason> {
    let mut buf = vec![0; BUFFER_SIZE];
    let mut stdin = Async::new(stdin())?;
    let mut uart = Async::new(uart)?;

    while let Some(result) = do_read(&stop, &mut stdin, &mut buf).await {
        let read_size = result?;
        if read_size == 0 {
            return Ok(Reason::EOF);
        }
        let control_o: u8 = 0x0f;
        let bytes_read = &buf[..read_size];
        if bytes_read.contains(&control_o) {
            return Ok(Reason::UserRequest);
        }
        uart.write_all(&bytes_read).await?;
        uart.flush().await?;
    }
    Ok(Reason::GotStop)
}

fn main() {
    let mut dev_name = "/dev/ttyUSB0".to_string();
    {
        let mut ap = ArgumentParser::new();
        ap.set_description("Connect to a serial line.");
        ap.refer(&mut dev_name)
            .add_argument("tty-device", Store, "Tty device to connect to");
        ap.parse_args_or_exit();
    }
    println!("Opening uart: {}", dev_name);

    let (uart_read, uart_write) = match create_uart(&dev_name) {
        Ok(x) => x,
        Err(e) => {
            eprintln!("Could not open uart: {}", e);
            return;
        }
    };

    // By using a local executor everything runs in this thread,
    // except for the IO reactor that is; that cannot be avoided with
    // smol's architecture.  But at least it means that we do not need
    // Arc or Send for this program's types, Rc is enough.
    let executor = smol::LocalExecutor::new();
    let stop = SendStop::new();
    let uart_write_loop_task = {
        let stop = stop.clone();
        executor.spawn(uart_write_loop(stop, uart_write))
    };
    let uart_read_loop_task = executor.spawn(uart_read_loop(stop, uart_read));
    let combined_fut = (uart_write_loop_task, uart_read_loop_task).join();
    let result = smol::block_on(executor.run(combined_fut));
    match result {
        (Ok(Reason::UserRequest), _) => println!("\r\nQuit on user request"),
        (_, Ok(Reason::EOF)) => println!("\r\nGot EOF from {}", dev_name),
        e => eprintln!("\r\nError: {:?}", e),
    };
}
