#![no_std]
#![no_main]

use alloc::vec::Vec;
use kernel_userspace::{
    net::Networking,
    service::{generate_tracking_number, get_public_service_id, ServiceMessageType},
    syscall::{self, exit, read_args, send_and_get_response_service_message, CURRENT_PID},
};

extern crate alloc;
#[macro_use]
extern crate userspace;
extern crate userspace_slaballoc;

#[export_name = "_start"]
pub extern "C" fn main() {
    let args = read_args();

    let mut args = args.split_whitespace();

    let cmd = args.next().unwrap();

    match cmd.to_uppercase().as_str() {
        "ARP" => {
            let mut ip = args.next().unwrap().split('.');
            let a = ip.next().unwrap();
            let b = ip.next().unwrap();
            let c = ip.next().unwrap();
            let d = ip.next().unwrap();
            let ip = lookup_ip(
                a.parse().unwrap(),
                b.parse().unwrap(),
                c.parse().unwrap(),
                d.parse().unwrap(),
            );
            println!("{a}.{b}.{c}.{d} = {ip:#X?}");
        }
        _ => println!("Unknown cmd"),
    }
    exit()
}

pub fn lookup_ip(a: u8, b: u8, c: u8, d: u8) -> Option<u64> {
    let mut buf = Vec::new();
    let networking = get_public_service_id("NETWORKING", &mut buf).unwrap();
    match send_and_get_response_service_message(
        &kernel_userspace::service::ServiceMessage {
            service_id: networking,
            sender_pid: *CURRENT_PID,
            tracking_number: generate_tracking_number(),
            destination: kernel_userspace::service::SendServiceMessageDest::ToProvider,
            message: ServiceMessageType::Networking(kernel_userspace::net::Networking::ArpRequest(
                a, b, c, d,
            )),
        },
        &mut buf,
    )
    .unwrap()
    .message
    {
        ServiceMessageType::Networking(Networking::ArpResponse(resp)) => {
            if let Some(_) = resp {
                return resp;
            }
        }
        _ => unimplemented!(),
    }
    None
}

#[panic_handler]
fn panic(i: &core::panic::PanicInfo) -> ! {
    println!("{}", i);
    exit()
}