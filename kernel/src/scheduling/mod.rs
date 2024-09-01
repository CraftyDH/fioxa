use crate::{cpu_localstorage::CPULocalStorageRW, time::pit::is_switching_tasks};

pub mod process;
pub mod taskmanager;

pub fn without_context_switch<F, R>(f: F) -> R
where
    F: FnOnce() -> R,
{
    // Check if we are switching tasks
    if is_switching_tasks() {
        unsafe {
            CPULocalStorageRW::inc_stay_scheduled();

            let tmp = f();

            CPULocalStorageRW::dec_stay_scheduled();

            tmp
        }
    } else {
        f()
    }
}
