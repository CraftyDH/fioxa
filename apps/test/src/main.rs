#![no_std]
#![no_main]

use kernel_userspace::{
    channel::Channel,
    sys::{
        syscall::{sys_process_spawn_thread, sys_read_args_string},
        types::{ObjectSignal, SyscallResult},
    },
};

extern crate alloc;
#[macro_use]
extern crate userspace;
extern crate userspace_slaballoc;

init_userspace!(main);

pub fn main() {
    let args = sys_read_args_string();
    let count: usize = if args.is_empty() {
        100000
    } else {
        args.parse().unwrap()
    };
    println!("Test elf, cnt = {count}");

    let (left, right) = Channel::new();
    let mut send_i = 0usize;
    let mut recv_i = 0usize;

    sys_process_spawn_thread(move || {
        loop {
            match right.read_val::<0, usize>(true) {
                Ok((val, _)) => {
                    right.write_val(&val, &[]).assert_ok();
                }
                Err(SyscallResult::ChannelClosed) => {
                    return;
                }
                Err(e) => panic!("Error got {e:?}"),
            }
        }
    });

    while send_i < count.min(1024) {
        left.write_val(&send_i, &[]).assert_ok();
        send_i += 1;
    }

    while recv_i < count {
        match left.read_val::<0, usize>(true) {
            Ok((val, _)) => {
                assert_eq!(val, recv_i);

                recv_i += 1;
                if recv_i.is_multiple_of(100000) {
                    println!("Received: {recv_i}");
                }

                if send_i < count {
                    left.write_val(&send_i, &[]).assert_ok();
                    send_i += 1;
                }
            }
            Err(SyscallResult::ChannelEmpty) => {
                left.handle().wait(ObjectSignal::READABLE).unwrap();
            }
            Err(e) => panic!("Error got {e:?}"),
        }
    }

    println!("Total received: {recv_i}");
}
