use core::{mem::MaybeUninit, u64};

use alloc::{
    collections::btree_map::BTreeMap,
    string::{String, ToString},
    sync::Arc,
    vec::Vec,
};
use hashbrown::HashMap;
use kernel_userspace::{
    channel::{channel_create_rs, channel_read, channel_write_rs, ChannelRead, ChannelReadResult},
    object::{object_wait_port_rs, KernelReference, KernelReferenceID, ObjectSignal},
    port::{port_create, port_wait, PortNotification, PortNotificationType},
    process::InitHandleMessage,
    service::deserialize,
};

use crate::{port::KPort, scheduling::process::Thread};

#[derive(Default)]
pub struct KObjectSignal {
    signal_status: ObjectSignal,
    signal_waiters: Vec<SignalWaiter>,
}

impl KObjectSignal {
    pub const fn new() -> Self {
        Self {
            signal_status: ObjectSignal::empty(),
            signal_waiters: Vec::new(),
        }
    }

    pub fn signal_status(&self) -> ObjectSignal {
        self.signal_status
    }

    pub fn wait(&mut self, waiter: SignalWaiter) {
        self.signal_waiters.push(waiter);
    }

    pub fn set_signal(&mut self, signal: ObjectSignal, status: bool) {
        let new = if status {
            self.signal_status | signal
        } else {
            self.signal_status & !signal
        };

        if new == self.signal_status {
            return;
        }

        self.signal_status = new;

        for waiter in self.signal_waiters.extract_if(|w| new.intersects(w.mask)) {
            match waiter.ty {
                SignalWaiterType::One(thread) => thread.wake(),
                SignalWaiterType::Port { port, key } => {
                    port.notify(PortNotification {
                        key,
                        ty: PortNotificationType::SignalOne {
                            trigger: waiter.mask,
                            signals: new,
                        },
                    });
                }
            }
        }
    }
}

pub struct SignalWaiter {
    pub ty: SignalWaiterType,
    pub mask: ObjectSignal,
}

pub enum SignalWaiterType {
    One(Arc<Thread>),
    Port { port: Arc<KPort>, key: u64 },
}

pub trait KObject {
    fn signals<T>(&self, f: impl FnOnce(&mut KObjectSignal) -> T) -> T;
}

pub fn init_handle_new_proc(channels: Vec<KernelReference>) {
    let port_handle = KernelReference::from_id(port_create());

    let mut chans: BTreeMap<u64, KernelReference> = BTreeMap::new();

    for (i, chan) in channels.into_iter().enumerate() {
        object_wait_port_rs(
            chan.id(),
            port_handle.id(),
            ObjectSignal::READABLE,
            (i + 10) as u64,
        );

        chans.insert((i + 10) as u64, chan);
    }

    let mut handles: HashMap<String, KernelReference> = HashMap::new();

    let mut notification = PortNotification {
        key: 0,
        ty: PortNotificationType::User(Default::default()),
    };
    loop {
        port_wait(port_handle.id(), &mut notification);
        match notification.ty {
            PortNotificationType::SignalOne { .. } => {
                let chan = chans.get(&notification.key).unwrap().id();
                if work_on_chan(chan, &mut handles, &mut chans, &port_handle) {
                    object_wait_port_rs(
                        chan,
                        port_handle.id(),
                        ObjectSignal::READABLE,
                        notification.key,
                    );
                } else {
                    chans.remove(&notification.key);
                }
            }
            _ => panic!("bad channel handle passed"),
        }
    }
}

fn work_on_chan(
    chan: KernelReferenceID,
    refs: &mut HashMap<String, KernelReference>,
    chans: &mut BTreeMap<u64, KernelReference>,
    port_handle: &KernelReference,
) -> bool {
    let mut data = Vec::with_capacity(100);
    let mut handles: MaybeUninit<KernelReferenceID> = MaybeUninit::uninit();

    loop {
        let mut read = ChannelRead {
            handle: chan,
            data: data.as_mut_ptr(),
            data_len: data.capacity(),
            handles: handles.as_mut_ptr().cast(),
            handles_len: 1,
        };
        match channel_read(&mut read) {
            ChannelReadResult::Ok => {
                unsafe { data.set_len(read.data_len) };
            }
            ChannelReadResult::Empty => return true,
            ChannelReadResult::Size => {
                if read.data_len > 0x1000 {
                    error!("got very large data {}", read.data_len);
                    return false;
                }
                data.reserve(read.data_len - data.len());
            }
            ChannelReadResult::Closed => return false,
        }

        let Ok(msg) = deserialize::<InitHandleMessage>(&data) else {
            warn!("bad message");
            return false;
        };

        match msg {
            InitHandleMessage::GetHandle(h) => match refs.get(h) {
                Some(handle) => {
                    let (left, right) = channel_create_rs();

                    channel_write_rs(handle.id(), &[true as u8], &[left.id()]);
                    channel_write_rs(chan, &[true as u8], &[right.id()]);
                }
                None => {
                    channel_write_rs(chan, &[false as u8], &[]);
                }
            },
            InitHandleMessage::PublishHandle(name) => {
                if read.handles_len != 1 {
                    warn!("bad handles len");
                    return false;
                }

                let publisher = unsafe { handles.assume_init() };
                let old = refs.insert(name.to_string(), KernelReference::from_id(publisher));

                channel_write_rs(chan, &[old.is_some() as u8], &[]);
            }
            InitHandleMessage::Clone => {
                let id = chans.last_key_value().unwrap().0 + 1;
                let (left, right) = channel_create_rs();
                assert!(chans.insert(id, left).is_none());
                object_wait_port_rs(chan, port_handle.id(), ObjectSignal::READABLE, id);

                channel_write_rs(chan, &[true as u8], &[right.id()]);
            }
        }
    }
}
