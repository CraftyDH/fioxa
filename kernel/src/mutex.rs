use core::sync::atomic::{AtomicBool, Ordering};

use lock_api::{GuardNoSend, RawMutex};

use crate::{cpu_localstorage::CPULocalStorageRW, time::pit::is_switching_tasks};

pub type Spinlock<T> = lock_api::Mutex<RawSpinlock, T>;
pub type SpinlockGuard<'a, T> = lock_api::MutexGuard<'a, RawSpinlock, T>;

pub struct RawSpinlock(AtomicBool);

unsafe impl RawMutex for RawSpinlock {
    const INIT: RawSpinlock = RawSpinlock(AtomicBool::new(false));

    // As we need to hold interrupts we cannot send the guard
    type GuardMarker = GuardNoSend;

    fn lock(&self) {
        if is_switching_tasks() {
            unsafe { CPULocalStorageRW::inc_hold_interrupts() };
        }

        while self
            .0
            .compare_exchange_weak(false, true, Ordering::Acquire, Ordering::Relaxed)
            .is_err()
        {
            // wait until the lock looks unlocked
            while self.is_locked() {
                core::hint::spin_loop();
            }
        }
    }

    fn try_lock(&self) -> bool {
        let switch = is_switching_tasks();
        if switch {
            unsafe { CPULocalStorageRW::inc_hold_interrupts() };
        }
        let lock = self
            .0
            .compare_exchange(false, true, Ordering::Acquire, Ordering::Relaxed)
            .is_ok();
        // Decrease stay scheduled if we didn't get the lock
        if !lock && switch {
            unsafe { CPULocalStorageRW::dec_hold_interrupts() };
        }
        lock
    }

    unsafe fn unlock(&self) {
        self.0.store(false, Ordering::Release);

        if is_switching_tasks() {
            // Safety: we increased it when it was locked
            CPULocalStorageRW::dec_hold_interrupts();
        }
    }

    fn is_locked(&self) -> bool {
        self.0.load(Ordering::Relaxed)
    }
}