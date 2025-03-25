use crate::cpu_localstorage::{CPULocalStorageRW, is_ls_enabled};

pub mod process;
pub mod taskmanager;

pub fn with_held_interrupts<F, R>(f: F) -> R
where
    F: FnOnce() -> R,
{
    // Check if we are switching tasks and not in the scheduler
    if is_ls_enabled() {
        unsafe {
            CPULocalStorageRW::inc_hold_interrupts();

            let tmp = f();

            CPULocalStorageRW::dec_hold_interrupts();

            tmp
        }
    } else {
        f()
    }
}
