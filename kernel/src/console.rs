use core::fmt::Write;

use alloc::{string::String, vec::Vec};
use input::keyboard::{
    KeyboardEvent,
    virtual_code::{Modifier, VirtualKeyCode},
};
use kernel_sys::{
    syscall::sys_process_spawn_thread,
    types::{ObjectSignal, SyscallResult},
};
use kernel_userspace::{
    backoff_sleep,
    channel::Channel,
    handle::Handle,
    input::InputServiceMessage,
    process::{INIT_HANDLE_SERVICE, ProcessHandle},
};

use crate::{
    bootfs::TERMINAL_ELF,
    cpu_localstorage::CPULocalStorageRW,
    elf::load_elf,
    scheduling::{process::ProcessReferences, with_held_interrupts},
    screen::gop::WRITER,
};

pub fn run_console() {
    let (stdin, cin) = Channel::new();
    let (stdout, cout) = Channel::new();
    let (stderr, cerr) = Channel::new();

    let mon_out = |chan: Channel| {
        sys_process_spawn_thread(move || {
            let mut read = Vec::with_capacity(0x1000);
            loop {
                chan.read::<0>(&mut read, false, true).unwrap();
                let s = String::from_utf8_lossy(&read);
                with_held_interrupts(|| {
                    let mut w = WRITER.get().unwrap().lock();
                    w.write_str(&s).unwrap();
                });
            }
        });
    };

    mon_out(stdout);
    mon_out(stderr);

    let keyboard = backoff_sleep(|| INIT_HANDLE_SERVICE.lock().get_service("INPUT:KB"));

    let mut kb_decoder = KBInputDecoder::new();

    sys_process_spawn_thread(move || {
        loop {
            let proc = load_elf(TERMINAL_ELF)
                .unwrap()
                .references(ProcessReferences::from_refs(&[
                    **INIT_HANDLE_SERVICE.lock().clone_init_service().handle(),
                    **cin.handle(),
                    **cout.handle(),
                    **cerr.handle(),
                ]))
                .build();

            let mut proc = unsafe {
                let thread = CPULocalStorageRW::get_current_task();
                ProcessHandle::from_handle(Handle::from_id(thread.process().add_value(proc.into())))
            };

            proc.blocking_exit_code();
            warn!("Terminal exited")
        }
    });

    loop {
        let str = kb_decoder.read(&keyboard);
        stdin.write(str.as_bytes(), &[]).assert_ok();
    }
}

pub struct KBInputDecoder {
    lshift: bool,
    rshift: bool,
    caps_lock: bool,
    num_lock: bool,
    str_buf: String,
}

impl KBInputDecoder {
    pub fn new() -> Self {
        Self {
            lshift: false,
            rshift: false,
            caps_lock: false,
            num_lock: false,
            str_buf: String::new(),
        }
    }

    pub fn read(&mut self, chan: &Channel) -> &str {
        self.str_buf.clear();
        loop {
            match chan.read_val::<0, _>(false) {
                Ok((ev, _)) => {
                    let chr = self.feed(ev);
                    self.str_buf.extend(chr);
                }
                Err(SyscallResult::ChannelEmpty) => {
                    if self.str_buf.is_empty() {
                        chan.handle()
                            .wait(ObjectSignal::READABLE | ObjectSignal::CHANNEL_CLOSED)
                            .unwrap();
                    } else {
                        return &self.str_buf;
                    }
                }
                Err(e) => panic!("{e:?}"),
            }
        }
    }

    pub fn feed(&mut self, ev: InputServiceMessage) -> Option<char> {
        match ev {
            kernel_userspace::input::InputServiceMessage::KeyboardEvent(scan_code) => {
                match scan_code {
                    KeyboardEvent::Up(VirtualKeyCode::Modifier(key)) => match key {
                        Modifier::LeftShift => self.lshift = false,
                        Modifier::RightShift => self.rshift = false,
                        _ => {}
                    },
                    KeyboardEvent::Up(_) => {}
                    KeyboardEvent::Down(VirtualKeyCode::Modifier(key)) => match key {
                        Modifier::LeftShift => self.lshift = true,
                        Modifier::RightShift => self.rshift = true,
                        Modifier::CapsLock => self.caps_lock = !self.caps_lock,
                        Modifier::NumLock => self.num_lock = !self.num_lock,
                        _ => {}
                    },
                    KeyboardEvent::Down(letter) => {
                        return Some(input::keyboard::us_keyboard::USKeymap::get_unicode(
                            letter,
                            self.lshift,
                            self.rshift,
                            self.caps_lock,
                            self.num_lock,
                        ));
                    }
                }
            }
            _ => todo!(),
        }
        None
    }
}
