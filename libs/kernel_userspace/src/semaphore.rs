use core::sync::atomic::{AtomicUsize, Ordering};

use kernel_sys::{
    syscall::{sys_futex_wait, sys_futex_wake},
    types::FutexFlags,
};

pub struct Semaphore {
    count: AtomicUsize,
}

impl Semaphore {
    pub const fn new(max: usize) -> Self {
        Semaphore {
            count: AtomicUsize::new(max),
        }
    }

    pub fn acquire<'a>(&'a self) -> SemaphoreGuard<'a> {
        let mut count = self.count.load(Ordering::Relaxed);
        loop {
            if count == 0 {
                sys_futex_wait(&self.count, FutexFlags::empty(), 0);
            } else {
                match self.count.compare_exchange_weak(
                    count,
                    count - 1,
                    Ordering::Acquire,
                    Ordering::Relaxed,
                ) {
                    Ok(_) => return SemaphoreGuard(self),
                    Err(c) => count = c,
                }
            }
        }
    }

    pub fn try_acquire<'a>(&'a self) -> Option<SemaphoreGuard<'a>> {
        let mut count = self.count.load(Ordering::Relaxed);
        loop {
            if count == 0 {
                return None;
            }
            match self.count.compare_exchange_weak(
                count,
                count - 1,
                Ordering::Acquire,
                Ordering::Relaxed,
            ) {
                Ok(_) => return Some(SemaphoreGuard(self)),
                Err(c) => count = c,
            }
        }
    }
}

pub struct SemaphoreGuard<'a>(&'a Semaphore);

impl Drop for SemaphoreGuard<'_> {
    fn drop(&mut self) {
        let old = self.0.count.fetch_add(1, Ordering::Release);

        if old == 0 {
            sys_futex_wake(&self.0.count, FutexFlags::empty(), usize::MAX);
        }
    }
}
