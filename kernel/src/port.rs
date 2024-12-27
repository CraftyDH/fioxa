use alloc::{collections::vec_deque::VecDeque, sync::Arc};
use kernel_userspace::port::PortNotification;

use crate::{
    cpu_localstorage::CPULocalStorageRW,
    mutex::Spinlock,
    scheduling::{
        process::{Thread, ThreadState},
        taskmanager::enter_sched,
    },
};

pub struct KPort {
    inner: Spinlock<KPortInner>,
}

pub struct KPortInner {
    queue: VecDeque<PortNotification>,
    waiters: VecDeque<Arc<Thread>>,
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

            let mut sched = thread.sched().lock();
            sched.state = ThreadState::Sleeping;
            this.waiters.push_back(thread.thread());
            drop(this);
            enter_sched(&mut sched);
        }
    }

    pub fn notify(&self, notif: PortNotification) {
        let mut this = self.inner.lock();
        this.queue.push_back(notif);
        if let Some(t) = this.waiters.pop_front() {
            t.wake();
        }
    }
}
