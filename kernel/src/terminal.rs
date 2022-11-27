use alloc::{
    string::{String, ToString},
    sync::Arc,
};
use conquer_once::spin::OnceCell;
use crossbeam_queue::{ArrayQueue, SegQueue};

use crate::{
    fs::{self, add_path, get_file_from_path, read_file},
    ps2::{
        keyboard,
        scancode::keys::{RawKeyCode, RawKeyCodeState},
        translate::{translate_raw_keycode, KeyCode},
    },
    syscall::yield_now,
    time,
};

pub static KEYPRESS_QUEUE: OnceCell<ArrayQueue<KeyCode>> = OnceCell::uninit();

pub fn terminal() {
    let keyboard_input = Arc::new(SegQueue::new());

    keyboard::subscribe(Arc::downgrade(&keyboard_input));

    let mut curr_line = String::new();

    let mut lshift = false;
    let mut rshift = false;
    let mut caps_lock = false;
    let mut num_lock = false;

    let mut cwd = String::from("/");

    loop {
        print!("=> ");
        'get_line: loop {
            while let Some(scan_code) = keyboard_input.pop() {
                match scan_code {
                    RawKeyCodeState::Up(code) => match code {
                        RawKeyCode::LeftShift => lshift = false,
                        RawKeyCode::RightShift => rshift = false,
                        _ => {}
                    },
                    RawKeyCodeState::Down(code) => match code {
                        RawKeyCode::LeftShift => lshift = true,
                        RawKeyCode::RightShift => rshift = true,
                        RawKeyCode::CapsLock => {
                            caps_lock = !caps_lock;
                        }
                        RawKeyCode::NumLock => {
                            num_lock = !num_lock;
                        }
                        RawKeyCode::Enter => {
                            print!("\n");
                            break 'get_line;
                        }
                        RawKeyCode::Backspace => {
                            if let Some(_) = curr_line.pop() {
                                print!("\x08");
                            }
                        }
                        _ => {
                            let shift = lshift | rshift;
                            match translate_raw_keycode(code, shift, caps_lock, num_lock) {
                                KeyCode::Unicode(key) => {
                                    curr_line.push(key);
                                    print!("{}", key);
                                }
                                KeyCode::SpecialCodes(_) => {
                                    curr_line.push('\0');
                                    print!("\0");
                                }
                            }
                        }
                    },
                }
            }
            yield_now()
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
        curr_line.clear();
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
