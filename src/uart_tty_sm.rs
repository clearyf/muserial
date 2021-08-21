use std::cell::Cell;
use std::os::unix::io::AsRawFd;
use std::rc::Rc;

use crate::reactor::Reactor;
use crate::transcript::Transcript;

const DEFAULT_READ_SIZE: usize = 1024;
const CONTROL_O: u8 = 0x0f;

#[derive(Copy, Clone, Debug)]
enum State {
    Reading(u64),
    Processing,
    Writing(u64),
    TearDown(u64),
    TornDown,
}

pub struct UartTtySM {
    tty: Box<dyn AsRawFd>,
    uart: Box<dyn AsRawFd>,
    uart_state: Cell<State>,
    tty_state: Cell<State>,
    transcript: Option<Transcript>,
}

impl UartTtySM {
    pub fn init_actions(
        reactor: &mut Reactor,
        tty: Box<dyn AsRawFd>,
        uart: Box<dyn AsRawFd>,
        transcript: Option<Transcript>,
    ) {
        let sm = Rc::new(UartTtySM {
            tty: tty,
            uart: uart,
            tty_state: Cell::new(State::Processing),
            uart_state: Cell::new(State::Processing),
            transcript: transcript,
        });
        let sm2 = Rc::clone(&sm);
        sm2.tty_state.set(State::Reading(reactor.read(
            sm2.tty.as_raw_fd(),
            vec![0; DEFAULT_READ_SIZE],
            Box::new(move |reactor, result, buf, _| tty_read_done(reactor, sm, result, buf)),
        )));
        let sm3 = Rc::clone(&sm2);
        sm3.uart_state.set(State::Reading(reactor.read(
            sm2.uart.as_raw_fd(),
            vec![0; DEFAULT_READ_SIZE],
            Box::new(move |reactor, result, buf, _| uart_read_done(reactor, sm2, result, buf)),
        )));
    }
}

fn tty_read_done(reactor: &mut Reactor, sm: Rc<UartTtySM>, result: i32, buf: Vec<u8>) {
    sm.tty_state.set(match sm.tty_state.get() {
        State::Reading(_) => State::Processing,
        State::TearDown(_) => return,
        e => panic!("tty_read_done in invalid state: {:?}", e),
    });
    if result <= 0 {
        panic!("Got error from tty read: {}", result);
    }
    if buf.contains(&CONTROL_O) {
        return start_uart_teardown(reactor, sm);
    } else {
        let sm2 = Rc::clone(&sm);
        let id = reactor.write(
            sm.uart.as_raw_fd(),
            buf,
            0,
            Box::new(move |reactor, result, buf, _| uart_write_done(reactor, sm2, result, buf)),
        );
        sm.tty_state.set(match sm.tty_state.get() {
            State::Processing => State::Writing(id),
            e => panic!("tty_read_done in invalid state: {:?}", e),
        });
    }
}

fn uart_write_done(reactor: &mut Reactor, sm: Rc<UartTtySM>, result: i32, mut buf: Vec<u8>) {
    sm.tty_state.set(match sm.tty_state.get() {
        State::Writing(_) => State::Processing,
        State::TearDown(_) => return,
        State::TornDown => return,
        e => panic!("uart_write_done in invalid state: {:?}", e),
    });
    if result == 0 {
        println!("\r\nPort disconnected\r");
        return start_uart_teardown(reactor, sm);
    } else if result < 0 {
        panic!("Got error from uart write: {}", result);
    } else if (result as usize) < buf.len() {
        let new_buf = buf.split_off(result as usize);
        let sm2 = Rc::clone(&sm);
        let id = reactor.write(
            sm.uart.as_raw_fd(),
            new_buf,
            0,
            Box::new(move |reactor, result, buf, _| uart_write_done(reactor, sm2, result, buf)),
        );
        sm.tty_state.set(match sm.tty_state.get() {
            State::Processing => State::Writing(id),
            e => panic!("uart_write_done in invalid state: {:?}", e),
        });
        return;
    }
    buf.resize(DEFAULT_READ_SIZE, 0);
    let sm2 = Rc::clone(&sm);
    let id = reactor.read(
        sm2.tty.as_raw_fd(),
        buf,
        Box::new(move |reactor, result, buf, _| tty_read_done(reactor, sm2, result, buf)),
    );
    sm.tty_state.set(match sm.tty_state.get() {
        State::Processing => State::Reading(id),
        e => panic!("uart_write_done in invalid state: {:?}", e),
    });
}

fn uart_read_done(reactor: &mut Reactor, sm: Rc<UartTtySM>, result: i32, buf: Vec<u8>) {
    sm.uart_state.set(match sm.uart_state.get() {
        State::Reading(_) => State::Processing,
        State::TearDown(_) => return,
        State::TornDown => return,
        e => panic!("uart_read_done in invalid state: {:?}", e),
    });
    if result == 0 {
        println!("\r\nPort disconnected\r");
        return start_uart_teardown(reactor, sm);
    }
    if result < 0 {
        println!("\r\nGot error from uart read: {}\r", result);
        return start_uart_teardown(reactor, sm);
    }
    if let Some(transcript) = &sm.transcript {
        transcript.log(reactor, &buf);
    }
    let sm2 = Rc::clone(&sm);
    let id = reactor.write(
        sm2.tty.as_raw_fd(),
        buf,
        0,
        Box::new(move |reactor, result, buf, _| tty_write_done(reactor, sm2, result, buf)),
    );
    sm.uart_state.set(match sm.uart_state.get() {
        State::Processing => State::Writing(id),
        e => panic!("uart_read_done in invalid state: {:?}", e),
    });
}

fn tty_write_done(reactor: &mut Reactor, sm: Rc<UartTtySM>, result: i32, mut buf: Vec<u8>) {
    sm.uart_state.set(match sm.uart_state.get() {
        State::Writing(_) => State::Processing,
        State::TearDown(_) => return,
        State::TornDown => return,
        e => panic!("tty_write_done in invalid state: {:?}", e),
    });
    if result == 0 {
        // Impossible; no tty available to write to as it's closed!
        return start_uart_teardown(reactor, sm);
    } else if result < 0 {
        panic!("Got error from tty write: {}", result);
    } else if (result as usize) < buf.len() {
        let new_buf = buf.split_off(result as usize);
        let sm2 = Rc::clone(&sm);
        let id = reactor.write(
            sm2.tty.as_raw_fd(),
            new_buf,
            0,
            Box::new(move |reactor, result, buf, _| tty_write_done(reactor, sm2, result, buf)),
        );
        sm.uart_state.set(match sm.uart_state.get() {
            State::Processing => State::Writing(id),
            e => panic!("uart_write_done in invalid state: {:?}", e),
        });
        return;
    }
    buf.resize(DEFAULT_READ_SIZE, 0);
    let sm2 = Rc::clone(&sm);
    let id = reactor.read(
        sm.uart.as_raw_fd(),
        buf,
        Box::new(move |reactor, result, buf, _| uart_read_done(reactor, sm2, result, buf)),
    );
    sm.uart_state.set(match sm.uart_state.get() {
        State::Processing => State::Reading(id),
        e => panic!("uart_write_done in invalid state: {:?}", e),
    });
}

fn start_uart_teardown(reactor: &mut Reactor, sm: Rc<UartTtySM>) {
    sm.tty_state.set(match sm.tty_state.get() {
        State::Processing => State::TornDown,
        State::Reading(id) => {
            let new_sm = sm.clone();
            let cancel_id = reactor.cancel(
                id,
                Box::new(move |reactor, result, user_data| {
                    handle_other_ev(reactor, new_sm, result, user_data)
                }),
            );
            State::TearDown(cancel_id)
        }
        State::Writing(id) => {
            let new_sm = sm.clone();
            let cancel_id = reactor.cancel(
                id,
                Box::new(move |reactor, result, user_data| {
                    handle_other_ev(reactor, new_sm, result, user_data)
                }),
            );
            State::TearDown(cancel_id)
        }
        e => panic!("start_uart_teardown in invalid state: {:?}", e),
    });
    sm.uart_state.set(match sm.uart_state.get() {
        State::Processing => State::TornDown,
        State::Reading(id) => {
            let new_sm = sm.clone();
            let cancel_id = reactor.cancel(
                id,
                Box::new(move |reactor, result, user_data| {
                    handle_other_ev(reactor, new_sm, result, user_data)
                }),
            );
            State::TearDown(cancel_id)
        }
        State::Writing(id) => {
            let new_sm = sm.clone();
            let cancel_id = reactor.cancel(
                id,
                Box::new(move |reactor, result, user_data| {
                    handle_other_ev(reactor, new_sm, result, user_data)
                }),
            );
            State::TearDown(cancel_id)
        }
        e => panic!("start_uart_teardown in invalid state: {:?}", e),
    });
    if let Some(transcript) = &sm.transcript {
        transcript.flush(reactor);
    }
}

fn handle_other_ev(_reactor: &mut Reactor, _: Rc<UartTtySM>, _result: i32, user_data: u64) {
    // TODO check?
    match user_data {
        _ => return,
    };
    // panic!("Got unknown user_data in handle_other_ev: {}", user_data)
}

#[cfg(test)]
mod tests {
    use crate::reactor::Reactor;
    use crate::uart_tty_sm::{UartTtySM, CONTROL_O};
    use libc::{socketpair, AF_UNIX, SOCK_STREAM};
    use std::fs::File;
    use std::io::Result;
    use std::io::{Read, Write};
    use std::mem::drop;
    use std::os::unix::io::FromRawFd;
    use std::thread;
    use std::thread::JoinHandle;

    fn create_socketpair() -> (File, File) {
        let mut sockets = [0; 2];
        let res = unsafe { socketpair(AF_UNIX, SOCK_STREAM, 0, sockets.as_mut_ptr()) };
        if res != 0 {
            panic!("Couldn't create socketpair for test: {}", res);
        }
        unsafe { (File::from_raw_fd(sockets[0]), File::from_raw_fd(sockets[1])) }
    }

    fn start_reactor() -> (File, File, JoinHandle<Result<()>>) {
        let (local, local_test) = create_socketpair();
        let (tty, tty_test) = create_socketpair();
        let child = thread::spawn(move || {
            let mut reactor = Reactor::new(1).unwrap();
            UartTtySM::init_actions(
                &mut reactor,
                Box::new(local_test),
                Box::new(tty_test),
                None,
            );
            reactor.run()
        });
        (local, tty, child)
    }

    #[test]
    fn test_uartttysm() {
        let (mut local, mut tty, child) = start_reactor();

        const TEST_STRING: &[u8; 11] = b"Hello world";
        local.write_all(TEST_STRING).unwrap();
        let mut buf = [0; TEST_STRING.len()];
        let _read = tty.read_exact(&mut buf);
        assert_eq!(TEST_STRING.len(), buf.len());
        for i in 0..buf.len() {
            assert_eq!(TEST_STRING[i], buf[i]);
        }

        // should be interpreted as tty close on the other side.
        drop(tty);
        child
            .join()
            .expect("Couldn't join reactor thread")
            .expect("Reactor returned error");
    }

    #[test]
    fn test_uartttysm_ctrl_o_exit() {
        let (mut local, _, child) = start_reactor();
        const TEST_STRING: [u8; 1] = [CONTROL_O];
        local.write_all(&TEST_STRING).unwrap();
        child
            .join()
            .expect("Couldn't join reactor thread")
            .expect("Reactor returned error");
    }
}
