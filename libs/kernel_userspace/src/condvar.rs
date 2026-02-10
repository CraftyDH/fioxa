use core::sync::atomic::{AtomicUsize, Ordering};

use kernel_sys::{
    syscall::{sys_futex_wait, sys_futex_wake},
    types::FutexFlags,
};

use crate::mutex::MutexGuard;

#[derive(Default)]
pub struct Condvar {
    seq: AtomicUsize,
}

impl Condvar {
    pub const fn new() -> Self {
        Self {
            seq: AtomicUsize::new(0),
        }
    }

    pub fn wait<'a, T>(&self, guard: MutexGuard<'a, T>) -> MutexGuard<'a, T> {
        let seq = self.seq.load(Ordering::Relaxed);
        let mutex = MutexGuard::mutex(&guard);
        drop(guard);

        for _ in 0..100 {
            if self.seq.load(Ordering::Relaxed) != seq {
                return mutex.lock();
            }
            core::hint::spin_loop();
        }

        sys_futex_wait(&self.seq, FutexFlags::empty(), seq);

        mutex.lock()
    }

    pub fn notify_one(&self) {
        self.seq.fetch_add(1, Ordering::Relaxed);

        sys_futex_wake(&self.seq, FutexFlags::empty(), 1);
    }

    pub fn notify_all(&self) {
        self.seq.fetch_add(1, Ordering::Relaxed);

        sys_futex_wake(&self.seq, FutexFlags::empty(), usize::MAX);
    }
}
