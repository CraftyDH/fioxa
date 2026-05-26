use alloc::{string::String, sync::Arc, vec::Vec};
use fioxa_rpc::{registry_capnp, service::ServiceExecutor};
use hashbrown::HashMap;
use kernel_sys::{
    syscall::sys_process_spawn_thread,
    types::{ObjectSignal, SysPortNotification, SysPortNotificationValue},
};
use kernel_userspace::{channel::Channel, handle::Handle, mutex::Mutex};

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

#[derive(Default, Debug, Clone)]
struct Entry {
    sources: Vec<Arc<Handle>>,
    single_waiters: Vec<Channel>,
    subscribers: Vec<Channel>,
}

struct InitSharedData {
    handles: HashMap<String, Entry>,
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
                fioxa_rpc::server::RPCServer::new(chan, InitHandler { shared }).run()
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

impl fioxa_rpc::registery::Service for InitHandler {
    fn register<'a>(
        &mut self,
        req: fioxa_rpc::OwnedReader<'a, registry_capnp::register::Owned>,
        mut req_handles: ::alloc::vec::Vec<::kernel_userspace::handle::Handle>,
        _res: fioxa_rpc::OwnedBuilder<'a, registry_capnp::register_resp::Owned>,
        _res_handles: &'a mut fioxa_rpc::RPCHandleBuilder,
    ) -> Result<(), ::capnp::Error> {
        let name = req.get_name()?.to_str()?;
        trace!("register: {name}");

        let handle = req_handles.remove(req.get_handle()?.get_index() as usize);
        let mut shared = self.shared.lock();
        let e = shared.handles.entry_ref(name).or_default();

        while let Some(c) = e.single_waiters.pop() {
            if let Err(e) = c.write(&[], &[*handle]) {
                warn!("error sending {e:?}");
            }
        }

        e.subscribers.retain(|c| {
            if let Err(e) = c.write(&[], &[*handle]) {
                warn!("error sending {e:?}");
                return false;
            }
            true
        });

        e.sources.push(Arc::new(handle));
        Ok(())
    }

    fn get<'a>(
        &mut self,
        req: fioxa_rpc::OwnedReader<'a, registry_capnp::get::Owned>,
        _req_handles: ::alloc::vec::Vec<::kernel_userspace::handle::Handle>,
        res: fioxa_rpc::OwnedBuilder<'a, registry_capnp::get_resp::Owned>,
        res_handles: &'a mut fioxa_rpc::RPCHandleBuilder<'static>,
    ) -> Result<(), ::capnp::Error> {
        let name = req.get_name()?.to_str()?;
        trace!("get handle: {name}");

        let mut shared = self.shared.lock();
        let entry = shared.handles.entry_ref(name).or_default();

        match req.get_mode().which()? {
            registry_capnp::get::mode::Which::Any(any) => {
                if let Some(h) = entry.sources.first() {
                    let e = res.init_entries(1);
                    res_handles.add(e.get(0), h.clone());
                } else if any.get_blocking() {
                    let (l, r) = Channel::new();
                    entry.single_waiters.push(r);
                    res_handles.add(res.init_extra(), l.into_inner());
                }
            }
            registry_capnp::get::mode::Which::Stream(cont) => {
                let (l, r) = Channel::new();
                res_handles.add(res.init_extra(), l.into_inner());
                for s in &entry.sources {
                    if let Err(e) = r.write(&[], &[***s]) {
                        warn!("error sending {e:?}");
                        break;
                    }
                }

                if cont.get_continue() {
                    entry.subscribers.push(r);
                }
            }
        }

        Ok(())
    }
}
