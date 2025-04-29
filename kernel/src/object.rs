use core::u64;

use alloc::{string::String, sync::Arc, vec::Vec};
use hashbrown::HashMap;
use kernel_sys::{
    syscall::sys_process_spawn_thread,
    types::{ObjectSignal, SysPortNotification, SysPortNotificationValue},
};
use kernel_userspace::{
    channel::Channel,
    ipc::IPCChannel,
    process::{InitHandleServiceExecutor, InitHandleServiceImpl},
    service::Service,
};
use spin::Mutex;

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

struct InitSharedData {
    handles: HashMap<String, Service>,
}

pub fn init_handle_new_proc(mut channels: Vec<Channel>) {
    let shared = Arc::new(Mutex::new(InitSharedData {
        handles: HashMap::new(),
    }));

    while let Some(c) = channels.pop() {
        launch(c, shared.clone());
    }
}

fn launch(chan: Channel, shared: Arc<Mutex<InitSharedData>>) {
    sys_process_spawn_thread(move || {
        match InitHandleServiceExecutor::new(IPCChannel::from_channel(chan), InitHandler { shared })
            .run()
        {
            Ok(()) => (),
            Err(e) => warn!("error handling init service: {e}"),
        }
    });
}

struct InitHandler {
    shared: Arc<Mutex<InitSharedData>>,
}

impl InitHandleServiceImpl for InitHandler {
    fn get_service(&mut self, name: &str) -> Option<Channel> {
        let mut shared = self.shared.lock();
        let channel = shared.handles.get_mut(name)?;

        let (l, r) = Channel::new();
        channel.send_consumer(r);

        Some(l)
    }

    fn publish_service(&mut self, name: &str, handle: Channel) -> bool {
        let mut shared = self.shared.lock();
        let old = shared.handles.insert(
            name.into(),
            Service::from_channel(IPCChannel::from_channel(handle)),
        );
        old.is_some()
    }

    fn clone_init_service(&mut self) -> Channel {
        let (l, r) = Channel::new();
        launch(r, self.shared.clone());
        l
    }
}
