use crate::{
    cpu_localstorage::{is_task_mgr_schedule, set_is_task_mgr_schedule},
    time::pit::is_switching_tasks,
};

pub mod process;
pub mod taskmanager;

pub fn without_context_switch<F, R>(f: F) -> R
where
    F: FnOnce() -> R,
{
    // Check if we are switching tasks
    if is_switching_tasks() {
        // get current status
        let current = is_task_mgr_schedule();
        // if set, unset
        if current {
            set_is_task_mgr_schedule(false);
        }

        let tmp = f();

        // reenable if needed
        if current {
            set_is_task_mgr_schedule(true);
        }

        tmp
    } else {
        f()
    }
}
