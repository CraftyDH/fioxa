use crate::{cpu_localstorage::CPULocalStorageRW, time::pit::is_switching_tasks};

pub mod process;
pub mod taskmanager;

pub fn without_context_switch<F, R>(f: F) -> R
where
    F: FnOnce() -> R,
{
    // Check if we are switching tasks
    if is_switching_tasks() {
        // get current status
        let initial = CPULocalStorageRW::get_stay_scheduled();
        // prevent thread from being scheduled away
        CPULocalStorageRW::set_stay_scheduled(true);

        let tmp = f();

        // reset to what it was
        CPULocalStorageRW::set_stay_scheduled(initial);

        tmp
    } else {
        f()
    }
}
