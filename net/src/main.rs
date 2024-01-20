#![no_std]
#![no_main]

use alloc::vec::Vec;
use kernel_userspace::{
    net::{ArpResponse, IPAddr, NetworkingResp, NotSameSubnetError},
    service::{generate_tracking_number, get_public_service_id, make_message},
    syscall::{exit, read_args, send_and_get_response_service_message, CURRENT_PID},
};

extern crate alloc;
#[macro_use]
extern crate userspace;
extern crate userspace_slaballoc;

#[export_name = "_start"]
pub extern "C" fn main() {
    let args = read_args();

    let mut args = args.split_whitespace();

    let cmd = args.next().expect("please provide args");

    match cmd.to_uppercase().as_str() {
        "ARP" => {
            let mut ip = args.next().unwrap().split('.');
            let a = ip.next().unwrap();
            let b = ip.next().unwrap();
            let c = ip.next().unwrap();
            let d = ip.next().unwrap();
            match lookup_ip(IPAddr::V4(
                a.parse().unwrap(),
                b.parse().unwrap(),
                c.parse().unwrap(),
                d.parse().unwrap(),
            )) {
                Ok(Some(mac)) => println!("{a}.{b}.{c}.{d} = {mac:#X?}"),
                Ok(None) => println!("pending answer, try again later"),
                Err(e) => println!("Failed to lookup arp because: {e}"),
            }
        }
        _ => println!("Unknown cmd"),
    }
    exit()
}

pub fn lookup_ip(ip: IPAddr) -> Result<Option<u64>, NotSameSubnetError> {
    let mut buf = Vec::new();
    let networking = get_public_service_id("NETWORKING", &mut buf).unwrap();
    match send_and_get_response_service_message(
        &kernel_userspace::service::ServiceMessageDesc {
            service_id: networking,
            sender_pid: *CURRENT_PID,
            tracking_number: generate_tracking_number(),
            destination: kernel_userspace::service::SendServiceMessageDest::ToProvider,
        },
        &make_message(&kernel_userspace::net::Networking::ArpRequest(ip), &mut buf),
    )
    .read(&mut buf)
    .unwrap()
    {
        NetworkingResp::ArpResponse(resp) => match resp {
            ArpResponse::Mac(mac) => return Ok(Some(mac)),
            ArpResponse::Pending(pend) => pend?,
        },
    }
    Ok(None)
}

#[panic_handler]
fn panic(i: &core::panic::PanicInfo) -> ! {
    println!("{}", i);
    exit()
}
