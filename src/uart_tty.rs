// use std::borrow::BorrowMut;
use std::cell::Cell;
// use std::cell::RefCell;
// use std::cell::RefMut;
use std::rc::Rc;

use std::fs::{File, OpenOptions};
use std::io::Result;
use std::os::unix::io::{AsRawFd, RawFd};

use crate::reactor::*;
use crate::transcript::*;
use crate::utility::*;

use libc::*;

const DEFAULT_READ_SIZE: usize = 1024;

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
    transcript: Option<Rc<Transcript>>,
}

impl UartTtySM {
    pub fn init_actions(
        reactor: &mut dyn ReactorSubmitter,
        tty: Box<dyn AsRawFd>,
        uart: Box<dyn AsRawFd>,
        transcript: Option<Transcript>,
    ) -> Rc<UartTtySM> {
        let sm = Rc::new(UartTtySM {
            tty: tty,
            uart: uart,
            tty_state: Cell::new(State::Processing),
            uart_state: Cell::new(State::Processing),
            transcript: transcript.map(|x| Rc::new(x)),
        });
        let sm_to_return = Rc::clone(&sm);
        let sm2 = Rc::clone(&sm);
        sm2.tty_state.set(State::Reading(reactor.submit_read(
            sm2.tty.as_raw_fd(),
            vec![0; DEFAULT_READ_SIZE],
            Box::new(move |reactor, result, buf, _| tty_read_done(reactor, sm, result, buf)),
        )));
        let sm3 = Rc::clone(&sm2);
        sm3.uart_state.set(State::Reading(reactor.submit_read(
            sm2.uart.as_raw_fd(),
            vec![0; DEFAULT_READ_SIZE],
            Box::new(move |reactor, result, buf, _| uart_read_done(reactor, sm2, result, buf)),
        )));
        sm_to_return
    }
}

fn tty_read_done(reactor: &mut dyn ReactorSubmitter, sm: Rc<UartTtySM>, result: i32, buf: Vec<u8>) {
    sm.tty_state.set(match sm.tty_state.get() {
        State::Reading(_) => State::Processing,
        State::TearDown(_) => return,
        e => panic!("tty_read_done in invalid state: {:?}", e),
    });
    if result < 0 {
        panic!("Got error from tty read: {}", result);
    }
    let control_o: u8 = 0x0f;
    if buf.contains(&control_o) {
        return start_uart_teardown(reactor, sm);
    } else {
        let sm2 = Rc::clone(&sm);
        let id = reactor.submit_write(
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

fn uart_write_done(
    reactor: &mut dyn ReactorSubmitter,
    sm: Rc<UartTtySM>,
    result: i32,
    mut buf: Vec<u8>,
) {
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
        let id = reactor.submit_write(
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
    let id = reactor.submit_read(
        sm2.tty.as_raw_fd(),
        buf,
        Box::new(move |reactor, result, buf, _| tty_read_done(reactor, sm2, result, buf)),
    );
    sm.tty_state.set(match sm.tty_state.get() {
        State::Processing => State::Reading(id),
        e => panic!("uart_write_done in invalid state: {:?}", e),
    });
}

fn uart_read_done(
    reactor: &mut dyn ReactorSubmitter,
    sm: Rc<UartTtySM>,
    result: i32,
    buf: Vec<u8>,
) {
    sm.uart_state.set(match sm.uart_state.get() {
        State::Reading(_) => State::Processing,
        State::TearDown(_) => return,
        State::TornDown => return,
        e => panic!("uart_read_done in invalid state: {:?}", e),
    });
    if result == 0 {
        println!("\r\nPort disconnected\r");
        return start_uart_teardown(reactor, sm);
    } else if result < 0 {
        panic!("Got error from uart read: {}", result);
    }
    // This is wrapped in a large bufwriter, so writes to the
    // transcript should be every few seconds at most; such writes
    // should also be extremely fast on any kind of remotely
    // modern hw.
    if let Some(transcript) = &sm.transcript {
        log_to_transcript(reactor, &transcript, &buf);
    }
    let sm2 = Rc::clone(&sm);
    let id = reactor.submit_write(
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

fn tty_write_done(
    reactor: &mut dyn ReactorSubmitter,
    sm: Rc<UartTtySM>,
    result: i32,
    mut buf: Vec<u8>,
) {
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
        let id = reactor.submit_write(
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
    let id = reactor.submit_read(
        sm.uart.as_raw_fd(),
        buf,
        Box::new(move |reactor, result, buf, _| uart_read_done(reactor, sm2, result, buf)),
    );
    sm.uart_state.set(match sm.uart_state.get() {
        State::Processing => State::Reading(id),
        e => panic!("uart_write_done in invalid state: {:?}", e),
    });
}

fn start_uart_teardown(reactor: &mut dyn ReactorSubmitter, sm: Rc<UartTtySM>) {
    sm.tty_state.set(match sm.tty_state.get() {
        State::Processing => State::TornDown,
        State::Reading(id) => {
            let new_sm = sm.clone();
            let cancel_id = reactor.submit_cancel(
                id,
                Box::new(move |reactor, result, user_data| {
                    handle_other_ev(reactor, new_sm, result, user_data)
                }),
            );
            State::TearDown(cancel_id)
        }
        State::Writing(id) => {
            let new_sm = sm.clone();
            let cancel_id = reactor.submit_cancel(
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
            let cancel_id = reactor.submit_cancel(
                id,
                Box::new(move |reactor, result, user_data| {
                    handle_other_ev(reactor, new_sm, result, user_data)
                }),
            );
            State::TearDown(cancel_id)
        }
        State::Writing(id) => {
            let new_sm = sm.clone();
            let cancel_id = reactor.submit_cancel(
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
        start_transcript_teardown(reactor, transcript.clone());
    }
}

fn handle_other_ev(
    _reactor: &mut dyn ReactorSubmitter,
    _: Rc<UartTtySM>,
    _result: i32,
    user_data: u64,
) {
    // TODO check?
    match user_data {
        _ => return,
    };
    // panic!("Got unknown user_data in handle_other_ev: {}", user_data)
}

#[cfg(test)]
mod tests {
    use crate::uart_tty::*;
    use std::collections::HashMap;

    pub struct TestSubmitter {
        actions: Vec<Action>,
        next_id: u64,
    }

    impl TestSubmitter {
        fn new(next_id: u64) -> TestSubmitter {
            TestSubmitter {
                actions: vec![],
                next_id: next_id,
            }
        }

        #[cfg(test)]
        pub fn get_actions(&mut self) -> HashMap<u64, Action> {
            let mut map: HashMap<u64, Action> = HashMap::new();
            let mut actions = Vec::new();
            std::mem::swap(&mut self.actions, &mut actions);
            for action in actions {
                let user_data = match &action {
                    Action::Read(_, _, user_data, _) => user_data,
                    Action::Write(_, _, _, user_data, _) => user_data,
                    Action::Cancel(_, user_data, _) => user_data,
                };
                map.insert(*user_data, action);
            }
            map
        }
    }

    impl ReactorSubmitter for TestSubmitter {
        fn submit_read(&mut self, fd: i32, buf: Vec<u8>, callback: RWCallback) -> u64 {
            let current_id = self.next_id;
            self.next_id += 1;
            self.actions
                .push(Action::Read(fd, buf, current_id, callback));
            return current_id;
        }

        fn submit_write(
            &mut self,
            fd: i32,
            buf: Vec<u8>,
            offset: usize,
            callback: RWCallback,
        ) -> u64 {
            let current_id = self.next_id;
            self.next_id += 1;
            self.actions
                .push(Action::Write(fd, buf, offset, current_id, callback));
            return current_id;
        }

        fn submit_cancel(&mut self, id: u64, callback: CancelCallback) -> u64 {
            let current_id = self.next_id;
            self.next_id += 1;
            self.actions.push(Action::Cancel(id, current_id, callback));
            return current_id;
        }
    }

    fn check_read(action: &Action, expected_fd: i32, buf_len: usize) {
        match &action {
            Action::Read(fd, buf, _, _) => {
                assert_eq!(*fd, expected_fd);
                assert_eq!(buf.len(), buf_len);
            }
            _ => panic!("check_read"),
        };
    }

    // fn check_cancel(action: &Action, expected_cancel_data: u64) {
    //     match &action {
    //         Action::Cancel(cancel_data, _user_data, _) => {
    //             assert_eq!(*cancel_data, expected_cancel_data);
    //         }
    //         e => panic!("check_cancel: {:?}", e),
    //     };
    // }

    fn reply_action(
        reactor: &mut dyn ReactorSubmitter,
        action: Action,
        result: i32,
        buf: Vec<u8>,
        user_data: u64,
    ) {
        match action {
            Action::Read(_, _, _, callback) => callback(reactor, result, buf, user_data),
            Action::Write(_, _, _, _, callback) => callback(reactor, result, buf, user_data),
            _ => panic!("reply_action"),
        }
    }

    fn reply_cancel(
        reactor: &mut dyn ReactorSubmitter,
        action: Action,
        result: i32,
        user_data: u64,
    ) {
        match action {
            Action::Cancel(_, _, callback) => callback(reactor, result, user_data),
            _ => panic!("reply_cancel"),
        }
    }

    fn get_reading_id(state: &State) -> u64 {
        if let State::Reading(id) = state {
            *id
        }
        else {
            panic!("Wrong state in get_reading_id: {:?}", state);
        }
    }

    fn get_writing_id(state: &State) -> u64 {
        if let State::Writing(id) = state {
            *id
        }
        else {
            panic!("Wrong state in get_writing_id: {:?}", state);
        }
    }

    fn get_teardown_id(state: &State) -> u64 {
        if let State::TearDown(id) = state {
            *id
        }
        else {
            panic!("Wrong state in get_teardown_id: {:?}", state);
        }
    }

    #[test]
    fn test_uartttysm() {
        let mut reactor = TestSubmitter::new(1);

        let sm = UartTtySM::init_actions(&mut reactor, Box::new(42), None);

        let mut actions = reactor.get_actions();
        assert_eq!(actions.len(), 2);
        {
            let tty_read = get_reading_id(&sm.tty_state.get());
            check_read(&actions.get(&tty_read).unwrap(), sm2.tty.as_raw_fd(), DEFAULT_READ_SIZE);
        }
        {
            let uart_read = get_reading_id(&sm.uart_state.get());
            check_read(&actions.get(&uart_read).unwrap(), 42, DEFAULT_READ_SIZE);
        }
        // Now start feeding actions to tty
        {
            let tty_read = get_reading_id(&sm.tty_state.get());
            reply_action(&mut reactor, actions.remove(&tty_read).unwrap(), 3, vec![97, 98, 99], tty_read);
        }
        // Should have a new action in the reactor, extract it into actions
        for (k, v) in reactor.get_actions().drain() {
            actions.insert(k, v);
        }
        assert_eq!(actions.len(), 2);
        {
            let tty_write = get_writing_id(&sm.tty_state.get());
            check_write(&actions.get(&tty_write).unwrap(), 42, 3, 0);
        }
        {
            let tty_write = get_writing_id(&sm.tty_state.get());
            reply_action(&mut reactor, actions.remove(&tty_write).unwrap(), 3, vec![97, 98, 99], tty_write);
        }
        // Should have a new action in the reactor, extract it into actions
        for (k, v) in reactor.get_actions().drain() {
            actions.insert(k, v);
        }
        assert_eq!(actions.len(), 2);
        {
            let tty_read = get_reading_id(&sm.tty_state.get());
            check_read(&actions.get(&tty_read).unwrap(), sm2.tty.as_raw_fd(), DEFAULT_READ_SIZE);
        }

        // uart side
        {
            let uart_read = get_reading_id(&sm.uart_state.get());
            reply_action(&mut reactor, actions.remove(&uart_read).unwrap(), 3, vec![97, 98, 99], uart_read);
        }
        // Should have a new action in the reactor, extract it into actions
        for (k, v) in reactor.get_actions().drain() {
            actions.insert(k, v);
        }
        assert_eq!(actions.len(), 2);
        {
            let uart_write = get_writing_id(&sm.uart_state.get());
            check_write(&actions.get(&uart_write).unwrap(), sm2.tty.as_raw_fd(), 3, 0);
        }
        {
            let uart_write = get_writing_id(&sm.uart_state.get());
            reply_action(&mut reactor, actions.remove(&uart_write).unwrap(), 3, vec![97, 98, 99], uart_write);
        }
        // Should have a new action in the reactor, extract it into actions
        for (k, v) in reactor.get_actions().drain() {
            actions.insert(k, v);
        }
        assert_eq!(actions.len(), 2);
        {
            let uart_read = get_reading_id(&sm.uart_state.get());
            check_read(&actions.get(&uart_read).unwrap(), 42, DEFAULT_READ_SIZE);
        }

        // Tty read to quit
        {
            let tty_read = get_reading_id(&sm.tty_state.get());
            reply_action(&mut reactor, actions.remove(&tty_read).unwrap(), 1, vec![15], tty_read);
        }
        // Should have a new action in the reactor, extract it into actions
        for (k, v) in reactor.get_actions().drain() {
            actions.insert(k, v);
        }
        assert_eq!(actions.len(), 2);
        {
            let uart_cancel = get_teardown_id(&sm.uart_state.get());
            reply_cancel(&mut reactor, actions.remove(&uart_cancel).unwrap(), 0, uart_cancel);
        }
        // Should have no new actions in the reactor, extract it into actions
        for (k, v) in reactor.get_actions().drain() {
            actions.insert(k, v);
        }
        assert_eq!(actions.len(), 1);
        for (k, v) in &actions {
            println!("{} -> {:?}", k, v);
        }
        // {
        //     let tty_cancel = get_reading_id(&sm.tty_state.get());
        //     reply_cancel(&mut reactor, actions.remove(&tty_cancel).unwrap(), 0, tty_cancel);
        // }
        // // Should have no new actions in the reactor, extract it into actions
        // for (k, v) in reactor.get_actions().drain() {
        //     actions.insert(k, v);
        // }
        // assert!(actions.is_empty());
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
