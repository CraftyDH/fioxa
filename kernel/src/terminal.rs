use alloc::{
    string::{String, ToString},
    sync::Arc,
};
use crossbeam_queue::ArrayQueue;
use input::keyboard::{
    virtual_code::{Modifier, VirtualKeyCode},
    KeyboardEvent,
};

use crate::{
    elf::load_elf,
    fs::{self, add_path, get_file_from_path, read_file, read_file_sector, PartitionId},
    scheduling::taskmanager::TASKMANAGER,
    stream::{self, STREAMRef},
    time, KB_STREAM_ID,
};
use kernel_userspace::syscall::yield_now;

pub struct KBInputDecoder {
    stream: STREAMRef,
    lshift: bool,
    rshift: bool,
    caps_lock: bool,
    num_lock: bool,
}

impl KBInputDecoder {
    pub fn new(stream: STREAMRef) -> Self {
        Self {
            stream,
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
            if let Some(st_message) = stream::pop() {
                let scan_code: &KeyboardEvent =
                    unsafe { &*(&st_message.data as *const [u8] as *const KeyboardEvent) };

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
}

pub fn terminal() {
    stream::subscribe(*KB_STREAM_ID.get().unwrap());

    let mut cwd: String = String::from("/");
    let mut partiton_id = 0;

    let mut input: KBInputDecoder =
        KBInputDecoder::new(Arc::downgrade(&Arc::new(ArrayQueue::new(1))));

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
                prints!("ECHO!");
            }
            "tree" => tree(partiton_id.into(), &cwd, rest),
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
                for part in fs::PARTITION.lock().keys() {
                    println!("{}:", part.0)
                }
            }
            "ls" => {
                let path = add_path(&cwd, rest);
                if let Some(file) = get_file_from_path(partiton_id.into(), &path) {
                    match file.specialized {
                        fs::VFileSpecialized::Folder(files) => {
                            for f in files {
                                println!("{}", f.0);
                            }
                        }
                        fs::VFileSpecialized::File(_) => println!("Not a folder"),
                    }
                } else {
                    println!("ls: no such file or directory")
                }
            }
            "cd" => cwd = add_path(&cwd, rest),
            "cat" => {
                for file in rest.split_ascii_whitespace() {
                    let path = add_path(&cwd, file);
                    if let Some(file) = get_file_from_path(partiton_id.into(), &path) {
                        let mut buffer = [0u8; 512];
                        for i in 0.. {
                            match read_file_sector(file.location, i, &mut buffer) {
                                Some(len) => {
                                    print!("{}", String::from_utf8_lossy(&buffer[0..len]));
                                }
                                None => break,
                            }
                        }
                    }
                }
                println!()
            }
            "exec" => {
                let (prog, args) = rest.split_once(' ').unwrap_or_else(|| (rest, ""));

                let path = add_path(&cwd, prog);
                if let Some(file) = get_file_from_path(partiton_id.into(), &path) {
                    match file.specialized {
                        fs::VFileSpecialized::Folder(_) => println!("Not a file"),
                        fs::VFileSpecialized::File(_) => {
                            let buf = read_file(file.location);

                            let pid = load_elf(&buf, args.to_string());

                            while TASKMANAGER.lock().processes.contains_key(&pid) {
                                yield_now();
                            }
                        }
                    }
                } else {
                    println!("exec: no such file or directory")
                }
            }
            "uptime" => {
                let mut uptime = time::uptime() / 1000;
                let seconds = uptime % 60;
                uptime /= 60;
                let minutes = uptime % 60;
                uptime /= 60;
                println!("Up: {:02}:{:02}:{:02}", uptime, minutes, seconds)
            }
            _ => {
                println!("{command}: command not found")
            }
        }
    }
}

pub fn tree(disk_id: PartitionId, cwd: &str, args: &str) {
    for sect in args.split(' ') {
        let path = fs::add_path(cwd, sect);
        println!("Path: {path}");
        if let Some(file) = get_file_from_path(disk_id, &path) {
            println!("{path}");
            fs::tree(file.location, String::new())
        } else {
            println!("{path} no such file or directory")
        }
    }
}
