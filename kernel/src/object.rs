use alloc::{string::String, sync::Arc, vec::Vec};
use hashbrown::HashMap;
use kernel_sys::{
    syscall::sys_process_spawn_thread,
    types::{ObjectSignal, SysPortNotification, SysPortNotificationValue},
};
use kernel_userspace::{
    channel::Channel,
    handle::Handle,
    ipc::IPCChannel,
    mutex::Mutex,
    process::{InitHandleServiceExecutor, InitHandleServiceImpl},
    service::ServiceExecutor,
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

struct InitSharedData {
    handles: HashMap<String, Handle>,
}

pub fn serve_init_service() -> Channel {
    let (left, right) = Channel::new();

    sys_process_spawn_thread(move || {
        let shared = Arc::new(Mutex::new(InitSharedData {
            handles: HashMap::new(),
        }));
        ServiceExecutor::from_channel(right, |chan| {
            let shared = shared.clone();
            sys_process_spawn_thread(|| {
                match InitHandleServiceExecutor::new(
                    IPCChannel::from_channel(chan),
                    InitHandler { shared },
                )
                .run()
                {
                    Ok(()) => (),
                    Err(e) => warn!("error handling init service: {e}"),
                }
            });
        })
        .run()
        .unwrap();
    });

    left
}

struct InitHandler {
    shared: Arc<Mutex<InitSharedData>>,
}

impl InitHandleServiceImpl for InitHandler {
    fn get_handle(&mut self, name: &str) -> Option<Handle> {
        trace!("get handle: {name}");
        let mut shared = self.shared.lock();
        let handle = shared.handles.get_mut(name)?;
        Some(handle.clone())
    }

    fn publish_handle(&mut self, name: &str, handle: Handle) -> bool {
        trace!("pub handle: {name}");
        let mut shared = self.shared.lock();
        let old = shared.handles.insert(name.into(), handle);
        old.is_some()
    }
}
