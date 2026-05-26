#![no_std]
#![no_main]

use alloc::vec::Vec;
use fioxa_rpc::echo_capnp;
use kernel_userspace::{channel::Channel, handle::Handle, sys::syscall::sys_process_spawn_thread};
use userspace::ARGS;

extern crate alloc;
#[macro_use]
extern crate userspace;
extern crate userspace_slaballoc;

init_userspace!(main);

pub fn main() {
    let args = ARGS.read_vec();
    let args = str::from_utf8(&args).unwrap();

    let count: usize = if args.is_empty() {
        100000
    } else {
        args.parse().unwrap()
    };
    println!("Test elf, cnt = {count}");

    let (left, right) = Channel::new();

    sys_process_spawn_thread(|| fioxa_rpc::server::RPCServer::new(right, Echo).run());

    let mut client = fioxa_rpc::client::RPCClient::new(left);

    for i in 0..count {
        let mut req = fioxa_rpc::echo::Echo::new_req();
        req.init().set_data(&i.to_ne_bytes());
        let res = client.send(&req.build()).unwrap();
        let mut data = res.get_reply().unwrap();
        let data = data.get_message().unwrap().get_data().unwrap();
        assert_eq!(data, &i.to_ne_bytes());

        if i % 10000 == 0 {
            println!("Received: {i}")
        }
    }

    println!("Total received: {count}");
}

struct Echo;

impl fioxa_rpc::echo::Service for Echo {
    fn echo<'a>(
        &mut self,
        req: fioxa_rpc::OwnedReader<'a, crate::echo_capnp::echo::Owned>,
        _req_handles: Vec<Handle>,
        mut res: fioxa_rpc::OwnedBuilder<'a, crate::echo_capnp::echo::Owned>,
        _res_handles: &'a mut fioxa_rpc::RPCHandleBuilder,
    ) -> Result<(), ::capnp::Error> {
        res.set_data(req.get_data()?);
        Ok(())
    }
}
