#![no_std]
#![no_main]

use kernel_userspace::{
    channel::Channel,
    process::get_handle,
    sys::{
        syscall::sys_read_args_string,
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
        usize::MAX
    } else {
        args.parse().unwrap()
    };
    println!("Test elf, cnt = {count}");

    let accepter = Channel::from_handle(get_handle("ACCEPTER").unwrap());

    let mut send_i = 0usize;
    let mut recv_i = 0usize;

    while send_i < 1024 {
        accepter.write_val(&send_i, &[]).assert_ok();
        send_i += 1;
    }

    while recv_i < count {
        match accepter.read_val::<0, usize>(true) {
            Ok((val, _)) => {
                assert_eq!(val, recv_i);
                recv_i += 1;
                if recv_i >= count {
                    break;
                }
                if send_i < count {
                    accepter.write_val(&send_i, &[]).assert_ok();
                    send_i += 1;
                }
            }
            Err(SyscallResult::ChannelEmpty) => {
                accepter.handle().wait(ObjectSignal::READABLE).unwrap();
            }
            _ => todo!(),
        }
    }
}
