#![no_std]
#![no_main]

use alloc::vec::Vec;
use kernel_userspace::{
    service::{generate_tracking_number, get_public_service_id, ServiceMessage},
    syscall::{send_service_message, CURRENT_PID},
};

extern crate alloc;
#[macro_use]
extern crate userspace;
extern crate userspace_bumpalloc;

#[export_name = "_start"]
pub extern "C" fn main() {
    print!("Hi");

    let mut buffer = Vec::new();
    let sid = get_public_service_id("ACCEPTER", &mut buffer).unwrap();

    for i in 0.. {
        send_service_message(&ServiceMessage {
            service_id: sid,
            sender_pid: *CURRENT_PID,
            tracking_number: generate_tracking_number(),
            destination: kernel_userspace::service::SendServiceMessageDest::ToProvider,
            message: kernel_userspace::service::ServiceMessageType::Ack,
        }, &mut buffer)
        .unwrap();
        // if i % 10000 == 0 {
        //     println!("SENDER {i}")
        // }
    }
}

#[panic_handler]
fn panic(i: &core::panic::PanicInfo) -> ! {
    println!("{}", i);
    loop {}
}
