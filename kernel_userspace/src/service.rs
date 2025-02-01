use core::ops::ControlFlow;

use alloc::{collections::btree_map::BTreeMap, vec::Vec};
use kernel_sys::types::ObjectSignal;
use serde::{Deserialize, Serialize};

use crate::{channel::Channel, message::MessageHandle, port::Port};

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

pub struct Service<A: FnMut() -> C, C, H: FnMut(&Channel, &mut C) -> ControlFlow<()>> {
    accepting_channel: Channel,
    port: Port,
    customers: BTreeMap<u64, (Channel, C)>,
    accepter: A,
    handler: H,
}

impl<A: FnMut() -> C, C, H: FnMut(&Channel, &mut C) -> ControlFlow<()>> Service<A, C, H> {
    pub fn new(name: &str, accepter: A, handler: H) -> Self {
        let (service, sright) = Channel::new();
        sright.handle().publish(name);

        let port = Port::new();

        service
            .handle()
            .wait_port(&port, ObjectSignal::READABLE, 0)
            .assert_ok();

        Self {
            accepting_channel: service,
            port,
            customers: BTreeMap::new(),
            accepter,
            handler,
        }
    }

    pub fn run(&mut self) {
        let mut data_buf = Vec::with_capacity(100);
        loop {
            let ev = self.port.wait().unwrap();
            if ev.key == 0 {
                let mut handles = self
                    .accepting_channel
                    .read::<1>(&mut data_buf, false, false)
                    .unwrap();

                assert!(handles.len() == 1);

                self.accepting_channel
                    .handle()
                    .wait_port(&self.port, ObjectSignal::READABLE, 0)
                    .assert_ok();

                let customer = Channel::from_handle(handles.pop().unwrap());

                let id = self
                    .customers
                    .last_key_value()
                    .map(|e| *e.0 + 1)
                    .unwrap_or(1);

                customer
                    .handle()
                    .wait_port(&self.port, ObjectSignal::READABLE, id)
                    .assert_ok();

                self.customers
                    .insert(id, (customer, self.accepter.call_mut(())));
            } else {
                let customer = self.customers.get_mut(&ev.key).unwrap();

                match self.handler.call_mut((&customer.0, &mut customer.1)) {
                    ControlFlow::Continue(_) => {
                        customer
                            .0
                            .handle()
                            .wait_port(&self.port, ObjectSignal::READABLE, ev.key)
                            .assert_ok();
                    }
                    ControlFlow::Break(_) => {
                        self.customers.remove(&ev.key);
                    }
                }
            }
        }
    }
}
