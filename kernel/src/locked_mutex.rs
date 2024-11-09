use crate::mutex::{Spinlock, SpinlockGuard};

// A trait that locks an arbitrary item behind a spin mutex
pub struct Locked<A> {
    inner: Spinlock<A>,
}

impl<A> Locked<A> {
    pub const fn new(inner: A) -> Self {
        Self {
            inner: Spinlock::new(inner),
        }
    }

    pub fn lock(&self) -> SpinlockGuard<A> {
        self.inner.lock()
    }
}
