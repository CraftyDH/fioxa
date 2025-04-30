#![no_std]
#![no_main]

use kernel_userspace::{
    ipc::IPCChannel,
    net::{IPAddr, NetService},
    sys::syscall::sys_read_args_string,
};

extern crate alloc;
#[macro_use]
extern crate userspace;
extern crate userspace_slaballoc;

init_userspace!(main);

pub fn main() {
    let args = sys_read_args_string();

    let mut args = args.split_whitespace();

    let cmd = args.next().expect("please provide args");

    match cmd.to_uppercase().as_str() {
        "ARP" => {
            let mut networking = NetService::from_channel(IPCChannel::connect("NETWORKING"));

            let mut ip = args.next().unwrap().split('.');
            let a = ip.next().unwrap();
            let b = ip.next().unwrap();
            let c = ip.next().unwrap();
            let d = ip.next().unwrap();
            match networking.arp_request(IPAddr::V4(
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
}
