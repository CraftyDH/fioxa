#![no_std]
#![no_main]

use core::time::Duration;

use anyhow::{Context, bail};
use fioxa_rpc::{
    client::RPCClient,
    fs::{GetChildren, add_path, stat_by_path, tree},
    fs_capnp,
    service::{connect_service, get_services},
};
use kernel_userspace::{
    channel::Channel,
    message::MessageHandle,
    mutex::Mutex,
    process::INIT_HANDLE_CHANNEL,
    sys::syscall::{sys_echo, sys_exit, sys_process_spawn_thread, sys_sleep},
};

extern crate alloc;
#[macro_use]
extern crate userspace;
extern crate userspace_slaballoc;

use alloc::{
    borrow::ToOwned, boxed::Box, collections::VecDeque, string::String, sync::Arc, vec::Vec,
};
use userspace::print::{STDERR_CHANNEL, STDIN_CHANNEL, STDOUT_CHANNEL, WRITER_STDOUT};

init_userspace!(main);

pub fn main() {
    let mut cwd: String = "/".to_owned();

    let mut input_history: VecDeque<Box<str>> = VecDeque::new();

    let mut input_buf = String::new();
    let mut input = input_buf.chars();

    let filesystems: Arc<Mutex<Vec<Channel>>> = Default::default();

    sys_process_spawn_thread({
        let filesystems = filesystems.clone();
        move || {
            get_services("FS", true, |chan| {
                filesystems.lock().push(chan);
            })
            .unwrap();
        }
    });

    let mut current_fs: Option<usize> = None;

    loop {
        match current_fs {
            Some(id) => print!("{id}:{cwd} "),
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

        match execute(&curr_line, &mut cwd, &filesystems, &mut current_fs) {
            Ok(()) => (),
            Err(e) => {
                if let Some(capnp) = e.downcast_ref::<capnp::Error>() {
                    eprintln!("capnp error: {capnp}")
                } else {
                    eprintln!("error: {e}")
                }
            }
        }
    }
}

fn execute(
    input: &str,
    cwd: &mut String,
    filesystems: &Mutex<Vec<Channel>>,
    current_fs: &mut Option<usize>,
) -> Result<(), anyhow::Error> {
    let (command, rest) = input.trim().split_once(' ').unwrap_or((input, ""));
    match command {
        "" => (),
        "pwd" => println!("{cwd}"),
        "echo" => println!("{rest}"),
        "disk" => {
            let c = rest.trim();
            let num = c.parse::<usize>();
            let fs_len = filesystems.lock().len();
            match num {
                Ok(num) => {
                    if num < fs_len {
                        *current_fs = Some(num);
                    } else {
                        println!("Disk {num} not in range 0..{fs_len}");
                    }
                }
                Err(_) => {
                    println!("Disks: 0..{fs_len}")
                }
            }
        }
        "ls" => {
            let fs_id = current_fs.context("no disk selected")?;
            let fs = filesystems.lock().get(fs_id).cloned();
            let fs = fs.expect("should be in range");

            let path = add_path(cwd, rest);

            match stat_by_path(fs, &path)? {
                fioxa_rpc::fs::StatResult::None => bail!("Invalid path"),
                fioxa_rpc::fs::StatResult::File(_) => bail!("This is a file"),
                fioxa_rpc::fs::StatResult::Folder(channel) => {
                    let mut folder =
                        RPCClient::<fs_capnp::FolderMessage>::new(connect_service(&channel)?);

                    let mut req = GetChildren::new_req();
                    req.init();
                    let r = folder.send(&req.build())?;
                    let mut r = r.get_reply()?;
                    let r = r.get_message()?;

                    if r.has_entries() {
                        let mut names: Vec<_> = r
                            .get_entries()?
                            .iter()
                            .flat_map(|e| e.get_name())
                            .flat_map(|e| e.to_str())
                            .collect();

                        numeric_sort::sort_unstable(&mut names);

                        for child in names {
                            println!("{child}")
                        }
                    }
                }
            }
        }
        "tree" => {
            let fs_id = current_fs.context("no disk selected")?;
            let fs = filesystems.lock().get(fs_id).cloned();
            let fs = fs.expect("should be in range");

            let path = add_path(cwd, rest);

            match stat_by_path(fs, &path)? {
                fioxa_rpc::fs::StatResult::None => bail!("Invalid path"),
                fioxa_rpc::fs::StatResult::File(_) => bail!("This is a file"),
                fioxa_rpc::fs::StatResult::Folder(channel) => {
                    let stdout = &mut *WRITER_STDOUT.lock();
                    tree(stdout, &channel, String::new())?;
                }
            }
        }
        "cd" => {
            let fs_id = current_fs.context("no disk selected")?;
            let fs = filesystems.lock().get(fs_id).cloned();
            let fs = fs.expect("should be in range");

            let path = add_path(cwd, rest);

            match stat_by_path(fs, &path)? {
                fioxa_rpc::fs::StatResult::None => bail!("Invalid path"),
                fioxa_rpc::fs::StatResult::File(_) => bail!("This is a file"),
                fioxa_rpc::fs::StatResult::Folder(_) => {
                    *cwd = path;
                }
            }
        }
        "cat" => {
            for file in rest.split_ascii_whitespace() {
                let fs_id = current_fs.context("no disk selected")?;
                let fs = filesystems.lock().get(fs_id).cloned();
                let fs = fs.expect("should be in range");

                let path = add_path(cwd, file);

                match stat_by_path(fs, &path)? {
                    fioxa_rpc::fs::StatResult::None => bail!("Invalid path"),
                    fioxa_rpc::fs::StatResult::Folder(_) => bail!("This is a folder"),
                    fioxa_rpc::fs::StatResult::File(file) => {
                        let mut file = RPCClient::<fioxa_rpc::fs_capnp::FileMessage>::new(
                            connect_service(&file)?,
                        );

                        let mut req = fioxa_rpc::fs::Size::new_req();
                        req.init();
                        let r = file.send(&req.build())?;
                        let mut r = r.get_reply()?;
                        let length = r.get_message()?.get_size() as usize;

                        const READ_SIZE: usize = 64 * 1024;
                        for start in (0..length).step_by(READ_SIZE) {
                            let len = (length - start).min(READ_SIZE);

                            let mut req = fioxa_rpc::fs::Read::new_req();
                            let mut b = req.init();
                            b.set_offset(start as u64);
                            b.set_len(len as u32);

                            let r = file.send(&req.build())?;
                            let mut r = r.get_reply()?;
                            let data = r.get_message()?.get_data()?;

                            WRITER_STDOUT.lock().write_raw(data).unwrap();
                        }
                    }
                }
            }
        }
        "write" => {
            let (file, data) = rest.split_once(" ").context("bad args")?;
            let fs_id = current_fs.context("no disk selected")?;
            let fs = filesystems.lock().get(fs_id).cloned();
            let fs = fs.expect("should be in range");

            let path = add_path(cwd, file);

            match stat_by_path(fs, &path)? {
                fioxa_rpc::fs::StatResult::None => bail!("Invalid path"),
                fioxa_rpc::fs::StatResult::Folder(_) => bail!("This is a folder"),
                fioxa_rpc::fs::StatResult::File(file) => {
                    let mut file =
                        RPCClient::<fioxa_rpc::fs_capnp::FileMessage>::new(connect_service(&file)?);
                    let mut req = fioxa_rpc::fs::Write::new_req();
                    let mut b = req.init();
                    b.set_offset(0);
                    b.set_data(data.as_bytes());
                    file.send(&req.build())?;
                }
            }
        }
        "append" => {
            let (file, data) = rest.split_once(" ").context("bad args")?;
            let fs_id = current_fs.context("no disk selected")?;
            let fs = filesystems.lock().get(fs_id).cloned();
            let fs = fs.expect("should be in range");

            let path = add_path(cwd, file);

            match stat_by_path(fs, &path)? {
                fioxa_rpc::fs::StatResult::None => bail!("Invalid path"),
                fioxa_rpc::fs::StatResult::Folder(_) => bail!("This is a folder"),
                fioxa_rpc::fs::StatResult::File(file) => {
                    let mut file =
                        RPCClient::<fioxa_rpc::fs_capnp::FileMessage>::new(connect_service(&file)?);
                    let mut req = fioxa_rpc::fs::Size::new_req();
                    req.init();
                    let r = file.send(&req.build())?;
                    let size = r.get_reply()?.get_message()?.get_size();

                    let mut req = fioxa_rpc::fs::Write::new_req();
                    let mut b = req.init();
                    b.set_offset(size);
                    b.set_data(data.as_bytes());
                    file.send(&req.build())?;
                }
            }
        }
        "exec" => {
            let fs_id = current_fs.context("no disk selected")?;

            let (prog, args) = rest.split_once(' ').unwrap_or((rest, ""));

            let args = MessageHandle::create(args.as_bytes());

            let fs = filesystems.lock().get(fs_id).cloned();
            let fs = fs.expect("should be in range");

            let path = add_path(cwd, prog);

            let mut proc = match stat_by_path(fs, &path)? {
                fioxa_rpc::fs::StatResult::None => bail!("Invalid path"),
                fioxa_rpc::fs::StatResult::Folder(_) => bail!("This is a folder"),
                fioxa_rpc::fs::StatResult::File(channel) => fioxa_rpc::elf::ElfClient::wellknown()
                    .spawn(
                        channel.handle(),
                        &[
                            INIT_HANDLE_CHANNEL.handle(),
                            STDIN_CHANNEL.handle(),
                            STDOUT_CHANNEL.handle(),
                            STDERR_CHANNEL.handle(),
                            args.handle(),
                        ],
                    ),
            }?;

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

    Ok(())
}
