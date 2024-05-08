#![no_std]
#![no_main]

use kernel_userspace::{
    backoff_sleep,
    elf::spawn_elf_process,
    event::receive_event,
    fs::{self, add_path, get_disks, read_file_sector, read_full_file, StatResponse},
    message::MessageHandle,
    object::{
        delete_reference, KernelObjectType, KernelReference, KernelReferenceID, REFERENCE_STDOUT,
    },
    process::ProcessExit,
    service::deserialize,
    socket::{socket_connect, socket_handle_get_event, socket_recv, SocketRecieveResult},
    syscall::{exit, sleep},
};

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
    socket: KernelReferenceID,
    socket_recv_ev: KernelReferenceID,
    lshift: bool,
    rshift: bool,
    caps_lock: bool,
    num_lock: bool,
    receive_buffer: Vec<u8>,
}

impl KBInputDecoder {
    pub fn new(socket: KernelReferenceID) -> Self {
        Self {
            socket,
            socket_recv_ev: socket_handle_get_event(
                socket,
                kernel_userspace::socket::SocketEvents::RecvBufferEmpty,
            ),
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
            receive_event(
                self.socket_recv_ev,
                kernel_userspace::event::ReceiveMode::LevelLow,
            );

            let (msg, ty) = match socket_recv(self.socket) {
                Ok(ok) => ok,
                Err(SocketRecieveResult::None) => continue,
                Err(SocketRecieveResult::EOF) => panic!("kb input channel eof"),
            };

            assert_eq!(ty, KernelObjectType::Message);

            let msg = MessageHandle::from_kref(KernelReference::from_id(msg));
            let size = msg.get_size();
            self.receive_buffer.resize(size, 0);
            msg.read(&mut self.receive_buffer);
            match deserialize(&self.receive_buffer).unwrap() {
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
        }
    }
}

#[export_name = "_start"]
pub extern "C" fn main() {
    let mut cwd: String = String::from("/");
    let mut partiton_id = 0u64;

    let mut buffer = Vec::new();
    let mut file_buffer = Vec::new();

    let keyboard_sid = backoff_sleep(|| socket_connect("INPUT:KB"));
    delete_reference(keyboard_sid);
    let keyboard_sid = backoff_sleep(|| socket_connect("INPUT:KB"));

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
                if curr_line.pop().is_some() {
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
            }
            "disk" => {
                let c = rest.trim();
                let c = c.chars().next();
                if let Some(chr) = c {
                    if let Some(n) = chr.to_digit(10) {
                        match fs::stat(n as usize, "/", &mut buffer) {
                            Ok(StatResponse::File(_)) => println!("cd: cannot cd into a file"),
                            Ok(StatResponse::Folder(_)) => {
                                partiton_id = n.into();
                            }
                            Err(e) => println!("cd: fs error: {e:?}"),
                        };

                        continue;
                    }
                }

                println!("Drives:");
                for part in get_disks(&mut buffer).unwrap().iter() {
                    println!("{}:", part)
                }
            }
            "ls" => {
                let path = add_path(&cwd, rest);

                match fs::stat(partiton_id as usize, path.as_str(), &mut buffer) {
                    Ok(StatResponse::File(_)) => println!("This is a file"),
                    Ok(StatResponse::Folder(c)) => {
                        for child in c.children {
                            println!("{child}")
                        }
                    }
                    Err(e) => println!("Error: {e:?}"),
                };
            }
            "cd" => cwd = add_path(&cwd, rest),
            "cat" => {
                for file in rest.split_ascii_whitespace() {
                    let path = add_path(&cwd, file);

                    let file = match fs::stat(partiton_id as usize, path.as_str(), &mut buffer) {
                        Ok(StatResponse::File(f)) => f,
                        Ok(StatResponse::Folder(_)) => {
                            println!("Not a file");
                            continue;
                        }
                        Err(e) => {
                            println!("Error: {e:?}");
                            break;
                        }
                    };

                    for i in 0..file.file_size / 512 {
                        let sect = match read_file_sector(
                            partiton_id as usize,
                            file.node_id,
                            i as u32,
                            &mut file_buffer,
                        ) {
                            Ok(s) => s,
                            Err(e) => {
                                println!("Error: {e:?}");
                                break;
                            }
                        };
                        if let Some(data) = sect {
                            data.read_into_vec(&mut file_buffer);
                            print!("{}", String::from_utf8_lossy(&file_buffer))
                        } else {
                            print!("Error reading");
                            break;
                        }
                    }
                }
            }
            "exec" => {
                let (prog, args) = rest.split_once(' ').unwrap_or((rest, ""));

                let path = add_path(&cwd, prog);

                let stat = fs::stat(partiton_id as usize, path.as_str(), &mut buffer);

                let file = match stat {
                    Ok(StatResponse::File(f)) => f,
                    Ok(StatResponse::Folder(_)) => {
                        println!("Not a file");
                        continue;
                    }
                    Err(e) => {
                        println!("Error: {e:?}");
                        continue;
                    }
                };
                println!("READING...");
                let contents =
                    match read_full_file(partiton_id as usize, file.node_id, &mut file_buffer) {
                        Ok(Some(c)) => c,
                        Ok(None) => {
                            println!("Failed to read file");
                            continue;
                        }
                        Err(e) => {
                            println!("Error: {e:?}");
                            continue;
                        }
                    };

                println!("SPAWNING...");

                // Clone stdout for now
                let proc =
                    spawn_elf_process(contents, args.as_bytes(), &[REFERENCE_STDOUT], &mut buffer);

                let mut proc = match proc {
                    Ok(p) => p,
                    Err(err) => {
                        println!("Error spawning: `{err}`");
                        continue;
                    }
                };

                match proc.blocking_exit_code() {
                    ProcessExit::Exited => (),
                    err => println!("Exited with error {err:?}"),
                }
            }
            // "uptime" => {
            //     let mut uptime = time::uptime() / 1000;
            //     let seconds = uptime % 60;
            //     uptime /= 60;
            //     let minutes = uptime % 60;
            //     uptime /= 60;
            //     println!("Up: {:02}:{:02}:{:02}", uptime, minutes, seconds)
            // }
            "sleep" => match rest.parse::<u64>() {
                Ok(n) => sleep(n),
                Err(e) => println!("sleep: {e:?}"),
            },
            "test" => {
                let test: [u8; 6] = [1, 2, 45, 29, 23, 45];

                let handle = MessageHandle::create(&test);
                let h2 = handle.clone();
                drop(handle);

                let res = h2.read_vec();
                assert_eq!(test, *res);

                println!("Passed test");
            }
            _ => {
                println!("{command}: command not found")
            }
        }
    }
}
