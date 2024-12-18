#![no_std]
#![no_main]

use core::mem::MaybeUninit;

use alloc::vec::Vec;
use kernel_userspace::{
    channel::{channel_read_val, channel_write_val, ChannelReadResult},
    object::{object_wait, ObjectSignal},
    process::get_handle,
    syscall::{exit, read_args},
};

extern crate alloc;
#[macro_use]
extern crate userspace;
extern crate userspace_slaballoc;

#[export_name = "_start"]
pub extern "C" fn main() {
    print!("Hi");

    let args = read_args();
    let count: usize = if args.is_empty() {
        usize::MAX
    } else {
        args.parse().unwrap()
    };

    let accepter = get_handle("ACCEPTER").unwrap();

    let mut send_i = 0usize;
    let mut recv_i = 0usize;

    while send_i < 1024 {
        channel_write_val(accepter, &send_i, &[]);
        send_i += 1;
    }

    while recv_i < count {
        let mut data: MaybeUninit<usize> = MaybeUninit::uninit();
        match channel_read_val(accepter, &mut data, &mut Vec::new()) {
            ChannelReadResult::Ok => unsafe {
                assert_eq!(data.assume_init(), recv_i);
                recv_i += 1;
                if recv_i >= count {
                    break;
                }
                if send_i < count {
                    channel_write_val(accepter, &send_i, &[]);
                    send_i += 1;
                }
            },
            ChannelReadResult::Empty => {
                object_wait(accepter, ObjectSignal::READABLE);
            }
            _ => todo!(),
        }
    }

    exit();
}

#[panic_handler]
fn panic(i: &core::panic::PanicInfo) -> ! {
    println!("{}", i);
    exit()
}
