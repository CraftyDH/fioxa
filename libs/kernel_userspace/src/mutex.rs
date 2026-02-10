use core::sync::atomic::{AtomicUsize, Ordering};

use kernel_sys::{
    syscall::{sys_futex_wait, sys_futex_wake},
    types::FutexFlags,
};
use lock_api::GuardSend;

pub type Mutex<T> = lock_api::Mutex<RawMutex, T>;
pub type MutexGuard<'a, T> = lock_api::MutexGuard<'a, RawMutex, T>;

/// This bit is set in the `state` of a `RawMutex` when that mutex is locked by some thread.
const LOCKED_BIT: usize = 0b01;
/// This bit is set in the `state` of a `RawMutex` just before parking a thread. A thread is being
/// parked if it wants to lock the mutex, but it is currently being held by some other thread.
const PARKED_BIT: usize = 0b10;

// Note: The implemention of this is modifed from parking_lot
pub struct RawMutex {
    state: AtomicUsize,
}

impl RawMutex {
    #[cold]
    fn lock_slow(&self) {
        let mut state = self.state.load(Ordering::Relaxed);
        let mut spin = 0;
        let mut set_flag = LOCKED_BIT;
        loop {
            // Grab the lock if it isn't locked, even if there is a queue on it
            if state & LOCKED_BIT == 0 {
                match self.state.compare_exchange_weak(
                    state,
                    state | set_flag,
                    Ordering::Acquire,
                    Ordering::Relaxed,
                ) {
                    Ok(_) => return,
                    Err(x) => state = x,
                }
                continue;
            }

            // If there is no queue, try spinning a few times
            if state & PARKED_BIT == 0 && spin < 100 {
                spin += 1;
                core::hint::spin_loop();
                state = self.state.load(Ordering::Relaxed);
                continue;
            }

            // Set the parked bit
            if state & PARKED_BIT == 0
                && let Err(x) = self.state.compare_exchange_weak(
                    state,
                    state | PARKED_BIT,
                    Ordering::Acquire,
                    Ordering::Relaxed,
                )
            {
                state = x;
                continue;
            }

            sys_futex_wait(&self.state, FutexFlags::empty(), state);
            state = self.state.load(Ordering::Relaxed);
            spin = 0;
            // There might be others waiting after us, so make sure we set PARKED so we wake them up on unlock
            set_flag |= PARKED_BIT;
        }
    }
}

unsafe impl lock_api::RawMutex for RawMutex {
    const INIT: Self = RawMutex {
        state: AtomicUsize::new(0),
    };

    type GuardMarker = GuardSend;

    fn lock(&self) {
        if self
            .state
            .compare_exchange_weak(0, LOCKED_BIT, Ordering::Acquire, Ordering::Relaxed)
            .is_err()
        {
            self.lock_slow();
        }
    }

    fn try_lock(&self) -> bool {
        let mut state = self.state.load(Ordering::Relaxed);
        loop {
            if state & LOCKED_BIT == LOCKED_BIT {
                return false;
            }
            match self.state.compare_exchange_weak(
                state,
                state | LOCKED_BIT,
                Ordering::Acquire,
                Ordering::Relaxed,
            ) {
                Ok(_) => return true,
                Err(x) => state = x,
            }
        }
    }

    unsafe fn unlock(&self) {
        let state = self.state.swap(0, Ordering::Release);
        if state & PARKED_BIT == PARKED_BIT {
            // wake 1, the wakee will then set PARKED again so eventually all wakers will wake
            sys_futex_wake(&self.state, FutexFlags::empty(), 1);
        }
    }

    fn is_locked(&self) -> bool {
        self.state.load(Ordering::Relaxed) & LOCKED_BIT == LOCKED_BIT
    }
}
