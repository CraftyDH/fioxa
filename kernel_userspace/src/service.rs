use core::{mem::MaybeUninit, ops::ControlFlow};

use alloc::{collections::btree_map::BTreeMap, vec::Vec};
use serde::{Deserialize, Serialize};

use crate::{
    backoff_sleep,
    channel::{
        channel_create_rs, channel_read_resize, channel_read_rs, channel_read_val,
        channel_write_rs, channel_write_val, ChannelReadResult,
    },
    message::MessageHandle,
    object::{object_wait_port_rs, KernelReference, KernelReferenceID, ObjectSignal},
    port::{port_create, port_wait_rs},
    process::{get_handle, publish_handle},
};

pub fn deserialize<'a, T: Deserialize<'a>>(buffer: &'a [u8]) -> Result<T, postcard::Error> {
    postcard::from_bytes(buffer)
}

pub fn make_message<T: Serialize>(msg: &T, buffer: &mut Vec<u8>) -> MessageHandle {
    let size =
        postcard::serialize_with_flavor(msg, postcard::ser_flavors::Size::default()).unwrap();
    unsafe {
        buffer.reserve(size);
        buffer.set_len(size);
    }
    let data = postcard::to_slice(msg, buffer).unwrap();
    MessageHandle::create(data)
}

pub fn serialize<'a, T: Serialize>(msg: &T, buffer: &'a mut Vec<u8>) -> &'a mut [u8] {
    let size =
        postcard::serialize_with_flavor(msg, postcard::ser_flavors::Size::default()).unwrap();
    unsafe {
        buffer.reserve(size);
        buffer.set_len(size);
    }
    postcard::to_slice(msg, buffer).unwrap()
}

pub fn make_message_new<T: Serialize>(msg: &T) -> MessageHandle {
    let data = postcard::to_allocvec(msg).unwrap();
    MessageHandle::create(&data)
}

pub struct Service<A: FnMut() -> C, C, H: FnMut(&KernelReference, &mut C) -> ControlFlow<()>> {
    accepting_channel: KernelReference,
    port: KernelReference,
    customers: BTreeMap<u64, (KernelReference, C)>,
    accepter: A,
    handler: H,
}

impl<A: FnMut() -> C, C, H: FnMut(&KernelReference, &mut C) -> ControlFlow<()>> Service<A, C, H> {
    pub fn new(name: &str, accepter: A, handler: H) -> Self {
        let (service, sright) = channel_create_rs();
        publish_handle(name, sright.id());

        let port = port_create();

        object_wait_port_rs(service.id(), port, ObjectSignal::READABLE, 0);
        Self {
            accepting_channel: service,
            port: KernelReference::from_id(port),
            customers: BTreeMap::new(),
            accepter,
            handler,
        }
    }

    pub fn run(&mut self) {
        let mut data_buf = Vec::with_capacity(100);
        let mut handles_buf = Vec::with_capacity(1);
        loop {
            let ev = port_wait_rs(self.port.id());
            if ev.key == 0 {
                match channel_read_rs(self.accepting_channel.id(), &mut data_buf, &mut handles_buf)
                {
                    crate::channel::ChannelReadResult::Ok => (),
                    _ => todo!(),
                }
                assert!(handles_buf.len() == 1);

                object_wait_port_rs(
                    self.accepting_channel.id(),
                    self.port.id(),
                    ObjectSignal::READABLE,
                    0,
                );

                let customer = KernelReference::from_id(handles_buf[0]);
                let id = self
                    .customers
                    .last_key_value()
                    .map(|e| *e.0 + 1)
                    .unwrap_or(1);
                object_wait_port_rs(customer.id(), self.port.id(), ObjectSignal::READABLE, id);

                self.customers
                    .insert(id, (customer, self.accepter.call_mut(())));
            } else {
                let customer = self.customers.get_mut(&ev.key).unwrap();

                match self.handler.call_mut((&customer.0, &mut customer.1)) {
                    ControlFlow::Continue(_) => {
                        object_wait_port_rs(
                            customer.0.id(),
                            self.port.id(),
                            ObjectSignal::READABLE,
                            ev.key,
                        );
                    }
                    ControlFlow::Break(_) => {
                        self.customers.remove(&ev.key);
                    }
                }
            }
        }
    }
}

pub struct SimpleService {
    handle: KernelReference,
}

impl SimpleService {
    pub fn new(handle: KernelReference) -> Self {
        Self { handle }
    }

    pub fn with_name(name: &str) -> Self {
        let handle = KernelReference::from_id(backoff_sleep(|| get_handle(name)));
        Self { handle }
    }

    pub fn send(&mut self, s: &[u8], handles: &[KernelReferenceID]) -> bool {
        channel_write_rs(self.handle.id(), s, handles)
    }

    pub fn send_val<S>(&mut self, s: &S, handles: &[KernelReferenceID]) -> bool {
        channel_write_val(self.handle.id(), s, handles)
    }

    pub fn recv(&mut self, data: &mut Vec<u8>, handles: &mut Vec<KernelReferenceID>) -> Option<()> {
        match channel_read_resize(self.handle.id(), data, handles) {
            ChannelReadResult::Ok => Some(()),
            ChannelReadResult::Closed => None,
            _ => todo!(),
        }
    }

    pub fn recv_val<R>(&mut self, handles: &mut Vec<KernelReferenceID>) -> Option<R> {
        let mut r = MaybeUninit::uninit();

        match channel_read_val(self.handle.id(), &mut r, handles) {
            crate::channel::ChannelReadResult::Ok => unsafe { Some(r.assume_init()) },
            crate::channel::ChannelReadResult::Closed => None,
            // The message was not the correct size_of<R>
            crate::channel::ChannelReadResult::Size => None,
            _ => todo!(),
        }
    }

    pub fn call(&mut self, buf: &mut Vec<u8>, handles: &mut Vec<KernelReferenceID>) -> Option<()> {
        self.send(buf, handles).then_some(())?;
        self.recv(buf, handles)
    }

    pub fn call_val<S, R>(&mut self, s: &S, handles: &mut Vec<KernelReferenceID>) -> R {
        self.send_val(s, handles);
        self.recv_val(handles).unwrap()
    }
}
