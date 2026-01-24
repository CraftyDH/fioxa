#![no_std]
#![no_main]

use core::time::Duration;

use kernel_userspace::{
    elf::ElfLoaderService,
    fs::{FSControllerService, FSFileId, FSFileType, FSService, add_path, stat_by_path},
    ipc::IPCChannel,
    message::MessageHandle,
    process::INIT_HANDLE_SERVICE,
    sys::syscall::{sys_echo, sys_exit, sys_sleep},
};

extern crate alloc;
#[macro_use]
extern crate userspace;
extern crate userspace_slaballoc;

use alloc::{borrow::ToOwned, boxed::Box, collections::VecDeque, string::String, vec::Vec};
use userspace::print::{STDERR_CHANNEL, STDIN_CHANNEL, STDOUT_CHANNEL, WRITER_STDOUT};

init_userspace!(main);

pub fn main() {
    let mut cwd: String = "/".to_owned();

    let mut input_history: VecDeque<Box<str>> = VecDeque::new();

    let mut input_buf = String::new();
    let mut input = input_buf.chars();

    let mut elf_loader = ElfLoaderService::from_channel(IPCChannel::connect("ELF_LOADER"));
    let mut fs_controller = FSControllerService::from_channel(IPCChannel::connect("FS_CONTROLLER"));

    let mut current_fs: Option<(usize, FSService, FSFileId)> = None;

    loop {
        match current_fs {
            Some((id, ..)) => print!("{id}:{cwd} "),
            None => print!(":{cwd} "),
        }

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
                if history_pos > 0
                    && let Some(chr) = input_history.get(history_pos - 1)
                {
                    curr_line.push_str(chr);
                    print!("{curr_line}")
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
                let num = c.parse::<usize>();
                let mut fs = fs_controller.get_filesystems(false);

                match num {
                    Ok(num) => match fs.nth(num) {
                        Some(fs) => {
                            let mut fs = FSService::from_channel(IPCChannel::from_channel(
                                fs.connect().unwrap(),
                            ));
                            let root = fs.stat_root().id;
                            current_fs = Some((num, fs, root));
                            cwd.clear();
                            cwd.push('/');
                        }
                        None => {
                            println!("Unknown disk")
                        }
                    },
                    Err(_) => {
                        println!("Drives:");
                        for (i, _) in fs.enumerate() {
                            println!("Disk {}", i)
                        }
                    }
                }
            }
            "ls" => {
                let Some((_, fs, root)) = &mut current_fs else {
                    println!("No disk selected");
                    continue;
                };

                let path = add_path(&cwd, rest);

                let Some(file) = stat_by_path(*root, &path, fs) else {
                    println!("Invalid path");
                    continue;
                };

                match file.file {
                    kernel_userspace::fs::FSFileType::File { .. } => {
                        println!("This is a file");
                    }
                    kernel_userspace::fs::FSFileType::Folder => {
                        let mut children = fs.get_children(file.id);
                        let (children, _) = children.access().unwrap();
                        let mut names: Vec<_> =
                            children.as_ref().unwrap().iter().map(|e| e.0).collect();
                        numeric_sort::sort_unstable(&mut names);
                        for child in names {
                            println!("{child}")
                        }
                    }
                }
            }
            "tree" => {
                let Some((_, fs, root)) = &mut current_fs else {
                    println!("No disk selected");
                    continue;
                };

                let path = add_path(&cwd, rest);

                let Some(file) = stat_by_path(*root, &path, fs) else {
                    println!("Invalid path");
                    continue;
                };

                match file.file {
                    kernel_userspace::fs::FSFileType::File { .. } => {
                        println!("This is a file");
                    }
                    kernel_userspace::fs::FSFileType::Folder => {
                        let stdout = &mut *WRITER_STDOUT.lock();
                        kernel_userspace::fs::tree(stdout, fs, file.id, String::new()).unwrap();
                    }
                }
            }
            "cd" => {
                let Some((_, fs, root)) = &mut current_fs else {
                    println!("No disk selected");
                    continue;
                };

                let new_cwd = add_path(&cwd, rest);
                match stat_by_path(*root, &new_cwd, fs) {
                    Some(file) => match file.file {
                        FSFileType::File { .. } => println!("Is a file"),
                        FSFileType::Folder => cwd = new_cwd,
                    },
                    None => {
                        println!("Invalid path")
                    }
                }
            }
            "cat" => {
                for file in rest.split_ascii_whitespace() {
                    let Some((_, fs, root)) = &mut current_fs else {
                        println!("No disk selected");
                        continue;
                    };

                    let path = add_path(&cwd, file);

                    let Some(file) = stat_by_path(*root, &path, fs) else {
                        println!("Invalid path");
                        continue;
                    };

                    match file.file {
                        kernel_userspace::fs::FSFileType::File { length } => {
                            let read_size = 64 * 1024;
                            for start in (0..length).step_by(read_size) {
                                let len = (length - start).min(read_size);

                                let mut read = fs.read_file(file.id, start, len);
                                let (buf, _) = read.access().unwrap();

                                WRITER_STDOUT
                                    .lock()
                                    .write_raw(buf.as_ref().unwrap())
                                    .unwrap();
                            }
                        }
                        kernel_userspace::fs::FSFileType::Folder => {
                            println!("This is a directory");
                        }
                    }
                }
            }
            "exec" => {
                let (prog, args) = rest.split_once(' ').unwrap_or((rest, ""));

                let Some((_, fs, root)) = &mut current_fs else {
                    println!("No disk selected");
                    continue;
                };

                let path = add_path(&cwd, prog);

                let Some(file) = stat_by_path(*root, &path, fs) else {
                    println!("Invalid path");
                    continue;
                };

                let contents = match file.file {
                    kernel_userspace::fs::FSFileType::File { length } => {
                        let mut read = fs.read_file(file.id, 0, length);
                        let (vec, _) = read.access().unwrap();
                        MessageHandle::create(vec.as_ref().unwrap())
                    }
                    kernel_userspace::fs::FSFileType::Folder => {
                        println!("This is a directory");
                        continue;
                    }
                };

                let proc = elf_loader.spawn(
                    &contents,
                    args.as_bytes(),
                    &[
                        INIT_HANDLE_SERVICE.0.handle(),
                        STDIN_CHANNEL.handle(),
                        STDOUT_CHANNEL.handle(),
                        STDERR_CHANNEL.handle(),
                    ],
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
