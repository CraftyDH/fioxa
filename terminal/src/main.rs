#![no_std]
#![no_main]

use kernel_userspace::{
    fs::{add_path, read_file_sector},
    proc::PID,
    syscall::{exit, read_args},
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

use core::mem::transmute;

use alloc::{
    string::{String, ToString},
    vec::Vec,
};
use input::keyboard::{
    virtual_code::{Modifier, VirtualKeyCode},
    KeyboardEvent,
};

use kernel_userspace::{
    fs::{
        get_disks, read_full_file, stat_file, ReadResponse, StatResponse, FS_READ_FULL_FILE,
        FS_STAT,
    },
    service::{
        generate_tracking_number, get_public_service_id, get_service_messages_sync,
        send_and_get_response_sync, SpawnProcess, SID,
    },
    syscall::{service_subscribe, yield_now},
};

pub struct KBInputDecoder {
    service: SID,
    lshift: bool,
    rshift: bool,
    caps_lock: bool,
    num_lock: bool,
}

impl KBInputDecoder {
    pub fn new(service: SID) -> Self {
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
        // let stream = self.stream.upgrade()?;

        loop {
            let msg = get_service_messages_sync(self.service);
            // let scan_code: &KeyboardEvent =
            //     unsafe { &*(&st_message.data as *const [u8] as *const KeyboardEvent) };

            let scan_code = msg.get_data_as::<KeyboardEvent>().unwrap();

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
                        letter.clone(),
                        self.lshift,
                        self.rshift,
                        self.caps_lock,
                        self.num_lock,
                    ));
                }
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

                let file = stat_file(fs_sid, partiton_id as usize, path.as_str());

                if file.get_message_header().data_type == FS_STAT {
                    let stat: StatResponse = file.get_data_as().unwrap();

                    match stat {
                        StatResponse::File(_) => println!("This is a file"),
                        StatResponse::Folder(c) => {
                            for child in c.children {
                                println!("{child}")
                            }
                        }
                    };
                } else {
                    println!("Invalid path")
                }
            }
            "cd" => cwd = add_path(&cwd, rest),
            "cat" => {
                for file in rest.split_ascii_whitespace() {
                    let path = add_path(&cwd, file);

                    let file = stat_file(fs_sid, partiton_id as usize, path.as_str());

                    if file.get_message_header().data_type == FS_STAT {
                        let stat: StatResponse = file.get_data_as().unwrap();

                        let file = match stat {
                            StatResponse::File(f) => f,
                            StatResponse::Folder(_) => {
                                println!("Not a file");
                                continue;
                            }
                        };

                        for i in 0..file.file_size / 512 {
                            let sect = read_file_sector(
                                fs_sid,
                                partiton_id as usize,
                                file.node_id,
                                i as u32,
                            );
                            let d = sect.get_data_as::<ReadResponse>().unwrap().data;
                            print!("{}", String::from_utf8_lossy(d))
                        }
                    } else {
                        println!("Error finding file")
                    }
                }
            }
            "exec" => {
                let (prog, args) = rest.split_once(' ').unwrap_or_else(|| (rest, ""));

                let path = add_path(&cwd, prog);

                let file = stat_file(fs_sid, partiton_id as usize, path.as_str());

                if file.get_message_header().data_type == FS_STAT {
                    let stat: StatResponse = file.get_data_as().unwrap();

                    let file = match stat {
                        StatResponse::File(f) => f,
                        StatResponse::Folder(_) => {
                            println!("Not a file");
                            continue;
                        }
                    };

                    let contents = read_full_file(fs_sid, partiton_id as usize, file.node_id);

                    if contents.get_message_header().data_type != FS_READ_FULL_FILE {
                        println!("Error reading file");
                        continue;
                    }

                    let contents_buffer = contents.get_data_as::<ReadResponse>().unwrap();

                    let spawn = send_and_get_response_sync(
                        SID(1),
                        kernel_userspace::service::MessageType::Request,
                        generate_tracking_number(),
                        1,
                        SpawnProcess {
                            elf: contents_buffer.data,
                            args: args.as_bytes(),
                        },
                        0,
                    );

                    let pid = PID(spawn.get_data_as::<u64>().unwrap());

                    // let pid = load_elf(&contents_buffer.data, args.as_bytes());
                    // while TASKMANAGER.lock().processes.contains_key(&pid) {
                    //     yield_now();
                    // }
                } else {
                    println!("Error finding file")
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
            _ => {
                println!("{command}: command not found")
            }
        }
    }
}