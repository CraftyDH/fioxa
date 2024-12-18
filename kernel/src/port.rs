use alloc::{collections::vec_deque::VecDeque, sync::Arc};
use kernel_userspace::port::PortNotification;

use crate::{
    cpu_localstorage::CPULocalStorageRW,
    mutex::Spinlock,
    scheduling::{process::ThreadHandle, taskmanager::block_task},
};

pub struct KPort {
    inner: Spinlock<KPortInner>,
}

pub struct KPortInner {
    queue: VecDeque<PortNotification>,
    waiters: VecDeque<Arc<ThreadHandle>>,
}

impl KPort {
    pub const fn new() -> KPort {
        Self {
            inner: Spinlock::new(KPortInner {
                queue: VecDeque::new(),
                waiters: VecDeque::new(),
            }),
        }
    }

    pub fn wait(&self) -> PortNotification {
        loop {
            let mut this = self.inner.lock();
            if let Some(n) = this.queue.pop_front() {
                return n;
            }

            let thread = unsafe { CPULocalStorageRW::get_current_task() };
            let handle = thread.handle();

            let status = handle.thread.lock();
            this.waiters.push_back(handle.clone());
            drop(this);
            block_task(status);
        }
    }

    pub fn notify(&self, notif: PortNotification) {
        let mut this = self.inner.lock();
        this.queue.push_back(notif);
        if let Some(t) = this.waiters.pop_front() {
            t.wake_up();
        }
    }
}
