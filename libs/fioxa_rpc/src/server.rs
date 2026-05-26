use core::marker::PhantomData;

use alloc::{string::ToString, vec::Vec};
use kernel_userspace::{
    channel::Channel,
    handle::Handle,
    sys::types::{Hid, SyscallError},
};
use log::warn;

use crate::{RPCHandleBuilder, rpc_capnp};

pub struct RPCServer<H, T> {
    channel: Channel,
    handler: H,
    _t: PhantomData<T>,
}

impl<T, H: RPCServiceHandler<T>> RPCServer<H, T> {
    pub fn new(channel: Channel, handler: H) -> Self {
        Self {
            channel,
            handler,
            _t: PhantomData,
        }
    }

    pub fn handle(&mut self) -> Result<(), anyhow::Error> {
        let mut buf = Vec::with_capacity(0x1000);
        loop {
            let req_handles = match self.channel.read::<32>(&mut buf, true, true) {
                Err(SyscallError::ChannelClosed) => return Ok(()),
                r => r?.into_iter().collect(),
            };

            let req = capnp::serialize::read_message_from_flat_slice(
                &mut &*buf,
                capnp::message::DEFAULT_READER_OPTIONS,
            )?;

            let mut res = capnp::message::Builder::new_default();
            let mut res_handles = RPCHandleBuilder::default();

            let ret = res.init_root::<rpc_capnp::return_::Builder>();
            let dispatch = self.handler.dispatch(
                req.get_root()?,
                req_handles,
                ret.init_results(),
                &mut res_handles,
            );

            match dispatch {
                Ok(()) => (),
                Err(e) => {
                    res = capnp::message::Builder::new_default();
                    res_handles = RPCHandleBuilder::new();

                    let ret = res.init_root::<rpc_capnp::return_::Builder>();
                    from_error(&e, ret.init_error());
                }
            }

            let res = capnp::serialize::write_message_segments_to_words(&res);

            let handles: Vec<Hid> = res_handles.0.iter().map(|h| ***h).collect();

            self.channel.write(&res, &handles)?;
            // ensure they stay alive during the write
            drop(res_handles);
        }
    }

    pub fn run(&mut self) {
        match self.handle() {
            Ok(()) => (),
            Err(e) => warn!("error handling service: {e}"),
        }
    }
}

pub trait RPCServiceHandler<T> {
    fn dispatch<'a>(
        &mut self,
        req: rpc_capnp::call::Reader<'a>,
        req_handles: Vec<Handle>,
        res: capnp::any_pointer::Builder<'a>,
        res_handles: &'a mut RPCHandleBuilder<'static>,
    ) -> Result<(), capnp::Error>;
}

fn from_error(error: &capnp::Error, mut builder: rpc_capnp::error::Builder) {
    let typ = match error.kind {
        ::capnp::ErrorKind::Failed => rpc_capnp::error::Type::Failed,
        ::capnp::ErrorKind::Overloaded => rpc_capnp::error::Type::Overloaded,
        ::capnp::ErrorKind::Disconnected => rpc_capnp::error::Type::Disconnected,
        ::capnp::ErrorKind::Unimplemented => rpc_capnp::error::Type::Unimplemented,
        ::capnp::ErrorKind::SettingDynamicCapabilitiesIsUnsupported => {
            rpc_capnp::error::Type::Unimplemented
        }
        _ => rpc_capnp::error::Type::Failed,
    };
    builder.set_type(typ);
    match error.kind {
        ::capnp::ErrorKind::Failed
        | ::capnp::ErrorKind::Overloaded
        | ::capnp::ErrorKind::Disconnected
        | ::capnp::ErrorKind::Unimplemented => {
            builder.set_reason(&error.extra);
        }
        _ => {
            // There is extra information in `error.kind` that is not
            // captured by `typ`. We call `error.to_string()` to allow that
            // information to be recorded in the `reason` field.
            builder.set_reason(error.to_string());
        }
    }
}
