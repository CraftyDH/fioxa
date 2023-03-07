use alloc::{
    string::{String, ToString},
    sync::Arc,
};
use input::keyboard::{
    virtual_code::{Modifier, VirtualKeyCode},
    KeyboardEvent,
};

use crate::{
    elf::load_elf,
    fs::{self, add_path, get_file_from_path, read_file},
    scheduling::taskmanager::TASKMANAGER,
    stream::{STREAMRef, STREAMS},
    syscall::yield_now,
    time,
};

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
        let stream = self.stream.upgrade()?;

        loop {
            if let Some(st_message) = stream.pop() {
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
    let keyboard_input = STREAMS.lock().get_mut("input:keyboard").unwrap().clone();
    let stdout_gop = STREAMS.lock().get("stdout").unwrap().clone();

    let mut cwd = String::from("/");

    let mut input = KBInputDecoder::new(Arc::downgrade(&keyboard_input));

    'outer: loop {
        print!("=> ");

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
            "echo" => println!("{rest}"),
            "tree" => tree(&cwd, rest),
            "ls" => {
                let path = add_path(&cwd, rest);
                if let Some(file) = get_file_from_path(&path) {
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
                    if let Some(file) = get_file_from_path(&path) {
                        let buf = read_file(file.location);
                        print!("{}", String::from_utf8_lossy(&buf));
                    }
                }
                println!()
            }
            "exec" => {
                let (prog, args) = rest.split_once(' ').unwrap_or_else(|| (rest, ""));

                let path = add_path(&cwd, prog);
                if let Some(file) = get_file_from_path(&path) {
                    match file.specialized {
                        fs::VFileSpecialized::Folder(_) => println!("Not a file"),
                        fs::VFileSpecialized::File(_) => {
                            let buf = read_file(file.location);

                            let pid = load_elf(&buf, args.to_string());

                            let stdout = TASKMANAGER
                                .lock()
                                .processes
                                .get(&pid)
                                .unwrap()
                                .stdout
                                .clone();

                            while TASKMANAGER.lock().processes.contains_key(&pid) {
                                if let Some(txt) = stdout.pop() {
                                    stdout_gop.force_push(txt);
                                }
                                if let Some(e) = keyboard_input.pop() {
                                    let scan_code: &KeyboardEvent = unsafe {
                                        &*(&e.data as *const [u8] as *const KeyboardEvent)
                                    };

                                    if let KeyboardEvent::Down(_) = scan_code {
                                        TASKMANAGER.lock().processes.remove(&pid);
                                        printsln!("Killed task");
                                        continue 'outer;
                                    }
                                }
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

pub fn tree(cwd: &str, args: &str) {
    for sect in args.split(' ') {
        let path = fs::add_path(cwd, sect);
        println!("Path: {path}");
        if let Some(file) = get_file_from_path(&path) {
            println!("{path}");
            fs::tree(file.location, String::new())
        } else {
            println!("{path} no such file or directory")
        }
    }
}
