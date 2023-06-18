#![no_std]
#![no_main]
#![feature(error_in_core)]

use kernel_userspace::{
    ids::ServiceID,
    service::{get_public_service_id, ServiceMessageType},
    syscall::{exit, receive_service_message_blocking, service_subscribe},
};
use terminal::script::{execute, Environment};

mod fns;

extern crate alloc;
#[macro_use]
extern crate userspace;
extern crate userspace_slaballoc;

#[panic_handler]
fn panic(i: &core::panic::PanicInfo) -> ! {
    println!("{}", i);
    exit()
}

use alloc::{
    string::{String, ToString},
    vec::Vec,
};
use input::keyboard::{
    virtual_code::{Modifier, VirtualKeyCode},
    KeyboardEvent,
};

pub struct KBInputDecoder {
    service: ServiceID,
    lshift: bool,
    rshift: bool,
    caps_lock: bool,
    num_lock: bool,
    receive_buffer: Vec<u8>,
}

impl KBInputDecoder {
    pub fn new(service: ServiceID) -> Self {
        Self {
            service,
            lshift: false,
            rshift: false,
            caps_lock: false,
            num_lock: false,
            receive_buffer: Default::default(),
        }
    }
}

impl Iterator for KBInputDecoder {
    type Item = char;

    fn next(&mut self) -> Option<Self::Item> {
        loop {
            let msg =
                receive_service_message_blocking(self.service, &mut self.receive_buffer).unwrap();

            match msg.message {
                ServiceMessageType::Input(
                    kernel_userspace::input::InputServiceMessage::KeyboardEvent(scan_code),
                ) => match scan_code {
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
                            letter.clone(),
                            self.lshift,
                            self.rshift,
                            self.caps_lock,
                            self.num_lock,
                        ));
                    }
                },
                _ => todo!(),
            }
        }
    }
}

#[export_name = "_start"]
pub extern "C" fn main() {
    let mut env = Environment::new(String::from("/"), 0);

    let fs_sid = env.add_service("FS").unwrap();
    let keyboard_sid = env.add_service("INPUT:KB").unwrap();
    let elf_loader_sid = env.add_service("ELF_LOADER").unwrap();

    env.services = Some(execute::Services {
        fs: fs_sid,
        keyboard: keyboard_sid,
        elf_loader: elf_loader_sid,
    });

    env.add_internal_fn("pwd", &fns::pwd);
    env.add_internal_fn("echo", &fns::echo);
    env.add_internal_fn("disk", &fns::disk);
    env.add_internal_fn("ls", &fns::ls);
    env.add_internal_fn("cd", &fns::cd);
    env.add_internal_fn("cat", &fns::cat);

    service_subscribe(keyboard_sid);
    let mut input: KBInputDecoder = KBInputDecoder::new(keyboard_sid);

    loop {
        print!("{}:{} ", env.partition_id, env.cwd);

        let mut curr_line = String::new();

        loop {
            let c = input.next().unwrap();
            if c == '\n' {
                println!();
                break;
            } else if c == '\x08' {
                if let Some(_) = curr_line.pop() {
                    print!("\x08");
                }
            } else {
                curr_line.push(c);
                print!("{c}");
            }
        }

        curr_line.push('\n');
        match execute(&curr_line, &mut env) {
            Ok(_) => {}
            Err(error) => println!("{}", error.to_string()),
        }
    }
}
