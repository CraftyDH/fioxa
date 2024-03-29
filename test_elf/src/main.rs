#![no_std]
#![no_main]

use alloc::vec::Vec;
use kernel_userspace::{
    service::{
        generate_tracking_number, get_public_service_id, make_message_new, ServiceMessageDesc,
    },
    syscall::{exit, send_service_message, CURRENT_PID},
};

extern crate alloc;
#[macro_use]
extern crate userspace;
extern crate userspace_slaballoc;

#[export_name = "_start"]
pub extern "C" fn main() {
    print!("Hi");

    let mut buffer = Vec::new();
    let sid = get_public_service_id("ACCEPTER", &mut buffer).unwrap();

    let msg = make_message_new(&());

    for i in 0.. {
        send_service_message(
            &ServiceMessageDesc {
                service_id: sid,
                sender_pid: *CURRENT_PID,
                tracking_number: generate_tracking_number(),
                destination: kernel_userspace::service::SendServiceMessageDest::ToProvider,
            },
            &msg,
        )
    }
}

#[panic_handler]
fn panic(i: &core::panic::PanicInfo) -> ! {
    println!("{}", i);
    exit()
}
