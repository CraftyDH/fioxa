#![no_std]
#![no_main]

use alloc::vec::Vec;
use kernel_userspace::{
    channel::Channel,
    net::{ArpResponse, IPAddr, NotSameSubnetError},
    process::get_handle,
    service::{deserialize, serialize},
    sys::syscall::{sys_exit, sys_read_args_string},
};

extern crate alloc;
#[macro_use]
extern crate userspace;
extern crate userspace_slaballoc;

#[unsafe(export_name = "_start")]
pub extern "C" fn main() {
    let args = sys_read_args_string();

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
    sys_exit()
}

pub fn lookup_ip(ip: IPAddr) -> Result<Option<u64>, NotSameSubnetError> {
    let networking = Channel::from_handle(get_handle("NETWORKING").unwrap());
    let mut buf = Vec::new();
    serialize(&kernel_userspace::net::Networking::ArpRequest(ip), &mut buf);
    networking.call::<0>(&mut buf, &[]).unwrap();

    match deserialize(&buf).unwrap() {
        ArpResponse::Mac(mac) => return Ok(Some(mac)),
        ArpResponse::Pending(pend) => pend?,
    }
    Ok(None)
}

#[panic_handler]
fn panic(i: &core::panic::PanicInfo) -> ! {
    println!("{}", i);
    sys_exit()
}
