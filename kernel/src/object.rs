use core::u64;

use alloc::{
    collections::btree_map::BTreeMap,
    string::{String, ToString},
    sync::Arc,
    vec::Vec,
};
use hashbrown::HashMap;
use kernel_sys::types::{
    ObjectSignal, SysPortNotification, SysPortNotificationValue, SyscallResult,
};
use kernel_userspace::{
    channel::Channel, port::Port, process::InitHandleMessage, service::deserialize,
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

        for waiter in self
            .signal_waiters
            .extract_if(.., |w| new.intersects(w.mask))
        {
            match waiter.ty {
                SignalWaiterType::One(thread) => thread.wake(),
                SignalWaiterType::Port { port, key } => {
                    port.notify(SysPortNotification {
                        key,
                        value: SysPortNotificationValue::SignalOne {
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

pub fn init_handle_new_proc(channels: Vec<Channel>) {
    let port_handle = Port::new();

    let mut chans: BTreeMap<u64, Channel> = BTreeMap::new();

    for (i, chan) in channels.into_iter().enumerate() {
        chan.handle()
            .wait_port(&port_handle, ObjectSignal::READABLE, (i + 10) as u64)
            .assert_ok();

        chans.insert((i + 10) as u64, chan);
    }

    let mut handles: HashMap<String, Channel> = HashMap::new();

    loop {
        let notification = port_handle.wait().unwrap();
        match notification.value {
            SysPortNotificationValue::SignalOne { .. } => {
                let chan = chans.remove(&notification.key).unwrap();
                if work_on_chan(&chan, &mut handles, &mut chans, &port_handle) {
                    let key = chans.last_key_value().unwrap().0 + 1;

                    chan.handle()
                        .wait_port(&port_handle, ObjectSignal::READABLE, key)
                        .assert_ok();
                    assert!(chans.insert(key, chan).is_none());
                }
            }
            _ => panic!("bad channel handle passed"),
        }
    }
}

fn work_on_chan(
    chan: &Channel,
    refs: &mut HashMap<String, Channel>,
    chans: &mut BTreeMap<u64, Channel>,
    port_handle: &Port,
) -> bool {
    let mut data = Vec::with_capacity(100);

    loop {
        let mut handles = match chan.read::<1>(&mut data, true, false) {
            Ok(h) => h,
            Err(SyscallResult::ChannelEmpty) => return true,
            Err(e) => {
                warn!("error recv: {e:?}");
                return false;
            }
        };

        let Ok(msg) = deserialize::<InitHandleMessage>(&data) else {
            warn!("bad message");
            return false;
        };

        match msg {
            InitHandleMessage::GetHandle(h) => match refs.get(h) {
                Some(handle) => {
                    let (left, right) = Channel::new();

                    handle.write(&[true as u8], &[**left.handle()]).assert_ok();
                    chan.write(&[true as u8], &[**right.handle()]).assert_ok();
                }
                None => {
                    chan.write(&[false as u8], &[]).assert_ok();
                }
            },
            InitHandleMessage::PublishHandle(name) => {
                if handles.len() != 1 {
                    warn!("bad handles len");
                    return false;
                }

                let old = refs.insert(
                    name.to_string(),
                    Channel::from_handle(handles.pop().unwrap()),
                );

                chan.write(&[old.is_some() as u8], &[]).assert_ok();
            }
            InitHandleMessage::Clone => {
                let key = chans.last_key_value().unwrap().0 + 1;
                let (left, right) = Channel::new();
                left.handle()
                    .wait_port(port_handle, ObjectSignal::READABLE, key)
                    .assert_ok();
                assert!(chans.insert(key, left).is_none());

                chan.write(&[true as u8], &[**right.handle()]).assert_ok();
            }
        }
    }
}
