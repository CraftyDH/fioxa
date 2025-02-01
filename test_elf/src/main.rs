#![no_std]
#![no_main]

use kernel_userspace::{
    channel::Channel,
    process::get_handle,
    sys::{
        syscall::{sys_exit, sys_read_args_string},
        types::{ObjectSignal, SyscallResult},
    },
};

extern crate alloc;
#[macro_use]
extern crate userspace;
extern crate userspace_slaballoc;

#[export_name = "_start"]
pub extern "C" fn main() {
    print!("Hi");

    let args = sys_read_args_string();
    let count: usize = if args.is_empty() {
        usize::MAX
    } else {
        args.parse().unwrap()
    };

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

    sys_exit();
}

#[panic_handler]
fn panic(i: &core::panic::PanicInfo) -> ! {
    println!("{}", i);
    sys_exit()
}
