use core::sync::atomic::{AtomicBool, Ordering};

// Do we probe the task manager for a new task?
pub static SWITCH_TASK: AtomicBool = AtomicBool::new(false);

pub fn start_switching_tasks() {
    SWITCH_TASK.store(true, Ordering::Relaxed)
}

pub fn is_switching_tasks() -> bool {
    SWITCH_TASK.load(Ordering::Relaxed)
}
