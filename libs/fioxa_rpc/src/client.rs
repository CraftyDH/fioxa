use core::marker::PhantomData;

use alloc::{format, vec::Vec};
use capnp::message::TypedBuilder;
use kernel_userspace::{
    channel::Channel,
    handle::Handle,
    sys::types::{Hid, SyscallError},
};

use crate::{RPCHandleBuilder, RPCHandles, RPCMethod, rpc_capnp};

pub struct RPCClient<I> {
    channel: Channel,
    _interface: PhantomData<I>,
}

impl<I: capnp::traits::HasTypeId> RPCClient<I> {
    pub const fn new(chan: Channel) -> Self {
        Self {
            channel: chan,
            _interface: PhantomData,
        }
    }

    pub fn into_inner(self) -> Channel {
        self.channel
    }

    pub fn send<M: RPCMethod<Interface = I>>(
        &mut self,
        call: &RPCReqBuilt<'_, M>,
    ) -> Result<RPCMessageReturn<M::Return>, SyscallError> {
        self.channel.write(&call.data, &call.handles)?;
        let mut res = Vec::new();
        let handles = self.channel.read::<32>(&mut res, true, true)?;
        Ok(RPCMessageReturn {
            data: res,
            handles: handles.into_iter().collect(),
            _t: PhantomData,
        })
    }
}

pub struct RPCReqBuilt<'a, M> {
    data: Vec<u8>,
    handles: Vec<Hid>,
    _handle_life: PhantomData<&'a M>,
}

pub struct RPCReqBuilder<M> {
    builder: TypedBuilder<crate::rpc_capnp::call::Owned>,
    _t: PhantomData<M>,
}

impl<M: RPCMethod> RPCReqBuilder<M> {
    pub fn new() -> Self {
        let message = capnp::message::Builder::new_default();
        let rpc = message.into_typed::<crate::rpc_capnp::call::Owned>();
        Self {
            builder: rpc,
            _t: PhantomData,
        }
    }

    pub fn init(&mut self) -> <M::Send as capnp::traits::Owned>::Builder<'_> {
        let mut r = self.builder.init_root();
        r.set_interface_id(<M::Interface as capnp::traits::HasTypeId>::TYPE_ID);
        r.set_method_id(M::ID);
        r.get_payload()
            .get_as()
            .expect("this should succeed as we just performed init")
    }

    pub fn get(&mut self) -> Result<<M::Send as capnp::traits::Owned>::Builder<'_>, capnp::Error> {
        self.builder.get_root()?.get_payload().get_as()
    }

    pub fn build(&self) -> RPCReqBuilt<'static, M> {
        RPCReqBuilt {
            data: capnp::serialize::write_message_to_words(self.builder.borrow_inner()),
            handles: Vec::new(),
            _handle_life: PhantomData,
        }
    }

    pub fn build_handles<'a>(&self, handles: &'a RPCHandleBuilder<'a>) -> RPCReqBuilt<'a, M> {
        RPCReqBuilt {
            data: capnp::serialize::write_message_to_words(self.builder.borrow_inner()),
            handles: handles.build(),
            _handle_life: PhantomData,
        }
    }
}

impl<M: RPCMethod> Default for RPCReqBuilder<M> {
    fn default() -> Self {
        Self::new()
    }
}

pub struct RPCMessageReturn<T> {
    data: Vec<u8>,
    handles: Vec<Handle>,
    _t: PhantomData<T>,
}

impl<'a, T> RPCMessageReturn<T> {
    pub fn get_reply(&'a self) -> Result<RPCMessageReturnReply<'a, T>, capnp::Error> {
        let reader = capnp::serialize::read_message_from_flat_slice(
            &mut &*self.data,
            capnp::message::DEFAULT_READER_OPTIONS,
        )?;
        Ok(RPCMessageReturnReply {
            reader,
            _t: PhantomData,
        })
    }

    pub fn get_handles(&self) -> &[Handle] {
        &self.handles
    }

    pub fn take_handles(&mut self) -> Vec<Handle> {
        core::mem::take(&mut self.handles)
    }

    pub fn take_handles_rpc(&mut self) -> RPCHandles {
        RPCHandles::new(self.take_handles().into_iter())
    }
}

pub struct RPCMessageReturnReply<'a, T> {
    reader: capnp::message::Reader<capnp::serialize::BufferSegments<&'a [u8]>>,
    _t: PhantomData<T>,
}

impl<'a, T: capnp::traits::Owned> RPCMessageReturnReply<'a, T> {
    pub fn get_message(&'a mut self) -> Result<T::Reader<'a>, capnp::Error> {
        let reader = self.reader.get_root::<rpc_capnp::return_::Reader>()?;
        match reader.which()? {
            rpc_capnp::return_::Which::Results(r) => Ok(r.get_as()?),
            rpc_capnp::return_::Which::Error(e) => Err(remote_exception_to_error(e?)),
        }
    }
}

fn remote_exception_to_error(exception: rpc_capnp::error::Reader) -> capnp::Error {
    let (kind, reason) = match (exception.get_type(), exception.get_reason()) {
        (Ok(rpc_capnp::error::Type::Failed), Ok(reason)) => (::capnp::ErrorKind::Failed, reason),
        (Ok(rpc_capnp::error::Type::Overloaded), Ok(reason)) => {
            (::capnp::ErrorKind::Overloaded, reason)
        }
        (Ok(rpc_capnp::error::Type::Disconnected), Ok(reason)) => {
            (::capnp::ErrorKind::Disconnected, reason)
        }
        (Ok(rpc_capnp::error::Type::Unimplemented), Ok(reason)) => {
            (::capnp::ErrorKind::Unimplemented, reason)
        }
        _ => (::capnp::ErrorKind::Failed, "(malformed error)".into()),
    };
    let reason_str = reason
        .to_str()
        .unwrap_or("<malformed utf-8 in error reason>");
    capnp::Error {
        extra: format!("remote exception: {reason_str}"),
        kind,
    }
}
