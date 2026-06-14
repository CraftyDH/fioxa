#![no_std]
#![no_main]

#[macro_use]
extern crate alloc;
#[macro_use]
extern crate userspace;
extern crate userspace_slaballoc;

#[macro_use]
extern crate log;

mod fat;

use fioxa_rpc::{client::RPCClient, service::connect_service};
use kernel_sys::types::{Hid, KernelObjectType};
use kernel_userspace::{channel::Channel, handle::Handle};

init_userspace!(main);

pub fn main() {
    let disk_ref = unsafe { Handle::from_id(Hid::from_usize(2).unwrap()) };
    assert_eq!(
        kernel_sys::syscall::sys_object_type(*disk_ref).unwrap(),
        KernelObjectType::Channel
    );

    let disk = RPCClient::<fioxa_rpc::disk_capnp::DiskMessage>::new(
        connect_service(&Channel::from_handle(disk_ref)).unwrap(),
    );

    fat::read_bios_block(disk);
}
