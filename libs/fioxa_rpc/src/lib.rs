#![no_std]

pub mod client;
pub mod elf;
pub mod fs;
pub mod interrupt;
pub mod pci;
pub mod server;
pub mod service;

use core::ops::Deref;

use alloc::{rc::Rc, sync::Arc, vec::Vec};
use kernel_userspace::{handle::Handle, sys::types::Hid};

extern crate alloc;

capnp::generated_code!(pub mod rpc_capnp);
capnp::generated_code!(pub mod disk_capnp);
capnp::generated_code!(pub mod echo_capnp);
capnp::generated_code!(pub mod elf_capnp);
capnp::generated_code!(pub mod fs_capnp);
capnp::generated_code!(pub mod interrupt_capnp);
capnp::generated_code!(pub mod net_capnp);
capnp::generated_code!(pub mod pci_capnp);
capnp::generated_code!(pub mod registry_capnp);

pub mod disk {
    crate::generate_rpc!(crate::disk_capnp::DiskMessage, Service;
        Read @ Read @ read(crate::disk_capnp::read::Owned) -> crate::disk_capnp::read_resp::Owned;
        Identify @ Identify @ identify(crate::disk_capnp::identify::Owned) -> crate::disk_capnp::read_resp::Owned;
        Write @ Write @ write(crate::disk_capnp::write::Owned) -> crate::disk_capnp::write_resp::Owned;
        Restrict @ Restrict @ restrict(crate::disk_capnp::restrict::Owned) -> crate::disk_capnp::restrict_resp::Owned;
    );
}

pub mod echo {
    crate::generate_rpc!(crate::echo_capnp::EchoMessage, Service;
        Echo @ Echo @ echo(crate::echo_capnp::echo::Owned) -> crate::echo_capnp::echo::Owned;
    );
}

pub mod net {
    use crate::net_capnp;
    crate::generate_rpc!(net_capnp::EthMessage, EthService;
        GetMac @ GetMac @ get_mac(net_capnp::eth_get_mac::Owned) -> net_capnp::mac_addr::Owned;
        SendPacket @ SendPacket @ send_packet(net_capnp::eth_send_packet::Owned) -> net_capnp::empty::Owned;
        ListenToPackets @ ListenToPackets @ listen(net_capnp::eth_listen_to_packets::Owned) -> net_capnp::empty::Owned;
    );

    crate::generate_rpc!(net_capnp::NetMessage, NetService;
        ArpRequest @ ArpRequest @ arp_request(net_capnp::arp_request::Owned) -> net_capnp::arp_reponse::Owned;
    );
}

pub mod registery {
    crate::generate_rpc!(crate::registry_capnp::RegistryMessage, Service;
        Register @ Register @ register(crate::registry_capnp::register::Owned) -> crate::registry_capnp::register_resp::Owned;
        Get @ Get @ get(crate::registry_capnp::get::Owned) -> crate::registry_capnp::get_resp::Owned
    );
}

pub type OwnedReader<'a, T> = <T as capnp::traits::Owned>::Reader<'a>;
pub type OwnedBuilder<'a, T> = <T as capnp::traits::Owned>::Builder<'a>;

pub trait RPCMethod {
    type Send: capnp::traits::Owned;
    type Return: capnp::traits::Owned;
    type Interface: capnp::traits::HasTypeId;
    const ID: u16;
}

pub enum HandleRef<'a> {
    Borrowd(&'a Handle),
    Owned(Handle),
    Rc(Rc<Handle>),
    Arc(Arc<Handle>),
}

impl<'a> From<&'a Handle> for HandleRef<'a> {
    fn from(value: &'a Handle) -> Self {
        HandleRef::Borrowd(value)
    }
}

impl From<Handle> for HandleRef<'static> {
    fn from(value: Handle) -> Self {
        HandleRef::Owned(value)
    }
}

impl From<Rc<Handle>> for HandleRef<'static> {
    fn from(value: Rc<Handle>) -> Self {
        HandleRef::Rc(value)
    }
}

impl From<Arc<Handle>> for HandleRef<'static> {
    fn from(value: Arc<Handle>) -> Self {
        HandleRef::Arc(value)
    }
}

impl Deref for HandleRef<'_> {
    type Target = Handle;

    fn deref(&self) -> &Self::Target {
        match self {
            HandleRef::Borrowd(handle) => handle,
            HandleRef::Owned(handle) => handle,
            HandleRef::Rc(handle) => handle,
            HandleRef::Arc(handle) => handle,
        }
    }
}

#[derive(Default)]

pub struct RPCHandleBuilder<'a>(Vec<HandleRef<'a>>);

impl<'a> RPCHandleBuilder<'a> {
    pub const fn new() -> Self {
        Self(Vec::new())
    }
    pub fn add(
        &mut self,
        mut builder: rpc_capnp::handle_index::Builder<'_>,
        handle: impl Into<HandleRef<'a>>,
    ) {
        let len = self.0.len();
        self.0.push(handle.into());
        builder.set_index(len.try_into().unwrap());
    }

    pub fn build(&self) -> Vec<Hid> {
        self.0.iter().map(|h| ***h).collect()
    }
}

pub struct RPCHandles(pub Vec<Option<Handle>>);

impl RPCHandles {
    pub fn new(vec: impl Iterator<Item = Handle>) -> Self {
        Self(vec.map(Some).collect())
    }

    pub fn get_handle(&self, handle: rpc_capnp::handle_index::Reader<'_>) -> Option<&Handle> {
        self.0.get(handle.get_index() as usize)?.as_ref()
    }

    pub fn take_handle(&mut self, handle: rpc_capnp::handle_index::Reader<'_>) -> Option<Handle> {
        self.0.get_mut(handle.get_index() as usize)?.take()
    }
}

#[macro_export]
macro_rules! generate_rpc {
    ($interface:ty, $service:ident; $($id:ident @ $struct:ident @ $serv:ident ($send:ty) -> $return:ty);+ $(;)?) => {
        $(
            pub struct $struct;

            impl $crate::RPCMethod for $struct {
                type Interface = $interface;
                type Send = $send;
                type Return = $return;
                const ID: u16 = <$interface>::$id as u16;
            }

            impl $struct {
                pub fn new_req() -> $crate::client::RPCReqBuilder::<Self> {
                    $crate::client::RPCReqBuilder::new()
                }
            }
        )*

        pub trait $service {
            $(
                fn $serv<'a>(&mut self, _req: $crate::OwnedReader<'a, $send>, _req_handles: ::alloc::vec::Vec<::kernel_userspace::handle::Handle>, _res: $crate::OwnedBuilder<'a, $return>, _res_handles: &'a mut $crate::RPCHandleBuilder<'static>) -> Result<(), ::capnp::Error> {
                    Err(capnp::Error::unimplemented("unimplemented".into()))
                }
            )*
        }

        impl<T: $service> $crate::server::RPCServiceHandler<$interface> for T {
            fn dispatch<'a>(
                &mut self,
                req: $crate::rpc_capnp::call::Reader<'a>,
                req_handles: ::alloc::vec::Vec<::kernel_userspace::handle::Handle>,
                res: ::capnp::any_pointer::Builder<'a>,
                res_handles: &'a mut $crate::RPCHandleBuilder<'static>,
            ) -> Result<(), capnp::Error> {
                if req.get_interface_id() != <$interface as capnp::traits::HasTypeId>::TYPE_ID {
                    return Err(capnp::Error::failed("interface id doesn't match".into()));
                }

                match req.get_method_id() {
                    $(
                        v if v == <$interface>::$id as u16 => {
                            self.$serv(
                                req.get_payload().get_as()?,
                                req_handles,
                                res.init_as(),
                                res_handles,
                            )
                        },
                    )*

                    _ => Err(capnp::Error::unimplemented("unknown method".into())),
                }
            }
        }
    };
}
