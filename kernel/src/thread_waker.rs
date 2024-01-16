use core::sync::atomic::AtomicUsize;

use alloc::sync::Arc;
use kernel_userspace::syscall::internal_kernel_waker_wait;

use crate::scheduling::process::Thread;

pub struct AtomicThreadWaker {
    state: AtomicUsize,
}

/// This is the resting state
pub const WAKER_RESTING: usize = 0;
/// Signal to prevent missed events
pub const WAKER_CHECK: usize = 1;

/// This works on the idea that the notifier always sets down to resting
/// thus you can `check` and if the `check` bit is still set after checking conditions
/// you know that you can sleep as you can't have lost a notification
impl AtomicThreadWaker {
    pub const fn new() -> Self {
        Self {
            state: AtomicUsize::new(WAKER_RESTING),
        }
    }

    pub fn wake(&self) {
        let old = self
            .state
            .swap(WAKER_RESTING, core::sync::atomic::Ordering::AcqRel);
        match old {
            WAKER_RESTING | WAKER_CHECK => (),
            arc => {
                let thread = unsafe { Arc::from_raw(arc as *const Thread) };
                thread.internal_wake();
            }
        }
    }

    pub fn check(&self) -> bool {
        let old = self
            .state
            .swap(WAKER_CHECK, core::sync::atomic::Ordering::AcqRel);
        match old {
            WAKER_RESTING => false,
            WAKER_CHECK => true,
            _ => panic!("waker is listening and should only be used by one thread"),
        }
    }

    pub fn set_waker(&self, waker: Arc<Thread>) -> bool {
        let w = Arc::into_raw(waker);
        let old = self.state.compare_exchange(
            WAKER_CHECK,
            w as usize,
            core::sync::atomic::Ordering::AcqRel,
            core::sync::atomic::Ordering::Acquire,
        );
        match old {
            Ok(_) => true,
            Err(WAKER_RESTING) => {
                // We failed to set, so drop the arc
                unsafe {
                    Arc::decrement_strong_count(w);
                }
                false
            }
            _ => panic!("Waker in bad state"),
        }
    }
}

/// Calls func, and sleeps thread until next event
pub fn atomic_waker_loop(waker: &AtomicThreadWaker, id: usize, mut func: impl FnMut()) -> ! {
    loop {
        func();
        if waker.check() {
            internal_kernel_waker_wait(id)
        }
    }
}
