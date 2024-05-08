#![no_std]
#![no_main]

use kernel_userspace::{
    service::make_message_new,
    socket::SocketHandle,
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
    let sid = SocketHandle::connect("ACCEPTER").unwrap();

    let msg = make_message_new(&());

    for _ in 0..count {
        sid.blocking_send(msg.kref());
    }

    exit();
}

#[panic_handler]
fn panic(i: &core::panic::PanicInfo) -> ! {
    println!("{}", i);
    exit()
}
