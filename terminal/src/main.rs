#![no_std]
#![no_main]

use core::time::Duration;

use kernel_userspace::{
    elf::spawn_elf_process,
    fs::{self, StatResponse, add_path, get_disks, read_file_sector, read_full_file},
    message::MessageHandle,
    process::clone_init_service,
    sys::syscall::{sys_echo, sys_exit, sys_sleep},
};

extern crate alloc;
#[macro_use]
extern crate userspace;
extern crate userspace_slaballoc;

use alloc::{boxed::Box, collections::VecDeque, string::String, vec::Vec};
use userspace::print::{STDERR_CHANNEL, STDIN_CHANNEL, STDOUT_CHANNEL, WRITER_STDOUT};

init_userspace!(main);

pub fn main() {
    let mut cwd: String = String::from("/");
    let mut partiton_id = 0u64;

    let mut buffer = Vec::new();
    let mut file_buffer = Vec::new();

    let mut input_history: VecDeque<Box<str>> = VecDeque::new();

    let mut input_buf = String::new();
    let mut input = input_buf.chars();

    loop {
        print!("{partiton_id}:{cwd} ");

        let mut curr_line = String::new();
        let mut history_pos: usize = 0;

        loop {
            let Some(c) = input.next() else {
                unsafe {
                    STDIN_CHANNEL
                        .read::<0>(input_buf.as_mut_vec(), true, true)
                        .unwrap()
                };
                input = input_buf.chars();
                continue;
            };
            if c == '\n' {
                if !curr_line.is_empty() {
                    input_history.push_front(curr_line.clone().into());
                    if input_history.len() > 1000 {
                        input_history.pop_back();
                    }
                }
                println!();
                break;
            } else if c == '\x08' {
                if curr_line.pop().is_some() {
                    print!("\x08");
                }
            } else if c == '\u{02193}' {
                history_pos = history_pos.saturating_sub(1);
                while curr_line.pop().is_some() {
                    print!("\x08");
                }
                if history_pos > 0 {
                    if let Some(chr) = input_history.get(history_pos - 1) {
                        curr_line.push_str(chr);
                        print!("{curr_line}")
                    }
                }
            } else if c == '\u{02191}' {
                if let Some(chr) = input_history.get(history_pos) {
                    history_pos += 1;
                    while curr_line.pop().is_some() {
                        print!("\x08");
                    }
                    curr_line.push_str(chr);
                    print!("{curr_line}")
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
            "pwd" => println!("{cwd}"),
            "echo" => println!("{rest}"),
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
                println!("Drives:");
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
                            WRITER_STDOUT.lock().write_raw(&file_buffer).unwrap();
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

                let proc = spawn_elf_process(
                    contents,
                    args.as_bytes(),
                    &[
                        **clone_init_service().handle(),
                        **STDIN_CHANNEL.handle(),
                        **STDOUT_CHANNEL.handle(),
                        **STDERR_CHANNEL.handle(),
                    ],
                    &mut buffer,
                );

                let mut proc = match proc {
                    Ok(p) => p,
                    Err(err) => {
                        println!("Error spawning: `{err}`");
                        continue;
                    }
                };

                proc.blocking_exit_code();
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
                Ok(n) => {
                    let act = sys_sleep(Duration::from_millis(n));
                    println!("sleep: slept for {act:?}");
                }
                Err(e) => println!("sleep: {e:?}"),
            },
            "test" => {
                let test: [u8; 6] = [1, 2, 45, 29, 23, 45];

                let handle = MessageHandle::create(&test);
                let h2 = handle.clone();
                drop(handle);

                let res = h2.read_vec();
                assert_eq!(test, *res);

                for i in 0..0x1000 {
                    assert_eq!(sys_echo(i), i);
                }

                println!("Passed test");
            }
            "exit" => {
                sys_exit();
            }
            _ => {
                println!("{command}: command not found")
            }
        }
    }
}
