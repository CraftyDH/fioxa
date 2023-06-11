#![no_std]
#![no_main]

use kernel_userspace::{
    fs::{self, add_path, get_disks, read_file_sector, read_full_file, StatResponse},
    ids::ServiceID,
    proc::PID,
    service::{
        generate_tracking_number, get_public_service_id, ServiceMessage, ServiceMessageType,
    },
    syscall::{
        exit, get_pid, send_and_wait_response_service_message, service_subscribe,
        wait_receive_service_message, CURRENT_PID,
    },
};

extern crate alloc;
#[macro_use]
extern crate userspace;
extern crate userspace_bumpalloc;

#[panic_handler]
fn panic(i: &core::panic::PanicInfo) -> ! {
    println!("{}", i);
    exit()
}

use alloc::string::{String, ToString};
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
}

impl KBInputDecoder {
    pub fn new(service: ServiceID) -> Self {
        Self {
            service,
            lshift: false,
            rshift: false,
            caps_lock: false,
            num_lock: false,
        }
    }
}

impl Iterator for KBInputDecoder {
    type Item = char;

    fn next(&mut self) -> Option<Self::Item> {
        loop {
            let msg = wait_receive_service_message(self.service);

            let message = msg.get_message().unwrap();

            match message.message {
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
    let mut cwd: String = String::from("/");
    let mut partiton_id = 0u64;

    let fs_sid = get_public_service_id("FS").unwrap();
    let keyboard_sid = get_public_service_id("INPUT:KB").unwrap();
    let elf_loader_sid = get_public_service_id("ELF_LOADER").unwrap();

    service_subscribe(keyboard_sid);

    let mut input: KBInputDecoder = KBInputDecoder::new(keyboard_sid);

    loop {
        print!("{partiton_id}:/ ");

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

        let (command, rest) = curr_line
            .trim()
            .split_once(' ')
            .unwrap_or((curr_line.as_str(), ""));
        match command {
            "" => (),
            "pwd" => println!("{}", cwd.to_string()),
            "echo" => {
                print!("ECHO!");
                unsafe { core::arch::asm!("cli") }
            }
            "disk" => {
                let c = rest.trim();
                let c = c.chars().next();
                if let Some(chr) = c {
                    if let Some(n) = chr.to_digit(10) {
                        partiton_id = n.into();
                        continue;
                    }
                }

                println!("Drives:");
                for part in get_disks(fs_sid) {
                    println!("{}:", part)
                }
            }
            "ls" => {
                let path = add_path(&cwd, rest);

                let stat = fs::stat(fs_sid, partiton_id as usize, path.as_str());

                match stat {
                    StatResponse::File(_) => println!("This is a file"),
                    StatResponse::Folder(c) => {
                        for child in c.children {
                            println!("{child}")
                        }
                    }
                    StatResponse::NotFound => println!("Invalid Path"),
                };
            }
            "cd" => cwd = add_path(&cwd, rest),
            "cat" => {
                for file in rest.split_ascii_whitespace() {
                    let path = add_path(&cwd, file);

                    let stat = fs::stat(fs_sid, partiton_id as usize, path.as_str());

                    let file = match stat {
                        StatResponse::File(f) => f,
                        StatResponse::Folder(_) => {
                            println!("Not a file");
                            continue;
                        }
                        StatResponse::NotFound => {
                            println!("File not found");
                            continue;
                        }
                    };

                    for i in 0..file.file_size / 512 {
                        let sect =
                            read_file_sector(fs_sid, partiton_id as usize, file.node_id, i as u32);
                        if let Some(data) = sect {
                            print!("{}", String::from_utf8_lossy(data.get_data()))
                        } else {
                            print!("Error reading");
                            break;
                        }
                    }
                }
            }
            "exec" => {
                let (prog, args) = rest.split_once(' ').unwrap_or_else(|| (rest, ""));

                let path = add_path(&cwd, prog);

                let stat = fs::stat(fs_sid, partiton_id as usize, path.as_str());

                let file = match stat {
                    StatResponse::File(f) => f,
                    StatResponse::Folder(_) => {
                        println!("Not a file");
                        continue;
                    }
                    StatResponse::NotFound => {
                        println!("File not found");
                        continue;
                    }
                };
                println!("READING...");
                let contents = read_full_file(fs_sid, partiton_id as usize, file.node_id).unwrap();

                println!("SPAWNING...");

                let resp = send_and_wait_response_service_message(&ServiceMessage {
                    service_id: elf_loader_sid,
                    sender_pid: *CURRENT_PID,
                    tracking_number: generate_tracking_number(),
                    destination: kernel_userspace::service::SendServiceMessageDest::ToProvider,
                    message: kernel_userspace::service::ServiceMessageType::ElfLoader(
                        contents.get_data(),
                        args.as_bytes(),
                    ),
                })
                .unwrap();

                // let pid = load_elf(&contents_buffer.data, args.as_bytes());
                // while TASKMANAGER.lock().processes.contains_key(&pid) {
                //     yield_now();
                // }
            }
            // "uptime" => {
            //     let mut uptime = time::uptime() / 1000;
            //     let seconds = uptime % 60;
            //     uptime /= 60;
            //     let minutes = uptime % 60;
            //     uptime /= 60;
            //     println!("Up: {:02}:{:02}:{:02}", uptime, minutes, seconds)
            // }
            _ => {
                println!("{command}: command not found")
            }
        }
    }
}
