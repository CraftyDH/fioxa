pub mod task;
pub mod taskmanager;

use core::sync::atomic::{AtomicU64, AtomicUsize, Ordering};
use spin::Mutex;
use x86_64::structures::idt::InterruptStackFrameValue;

use crate::assembly::registers::Registers;

// Start stack at this address
static STACK_ADDR: AtomicU64 = AtomicU64::new(0x100_000_000_000);
const STACK_SIZE: usize = 0x10000; // 64 Kb;

#[derive(Debug, Clone, Copy, PartialEq, Eq, PartialOrd, Ord)]
pub struct TaskID(usize);

impl TaskID {
    pub fn new() -> Self {
        static NEXT_ID: AtomicUsize = AtomicUsize::new(1);
        Self(NEXT_ID.fetch_add(1, Ordering::Relaxed))
    }
}

impl From<usize> for TaskID {
    fn from(id: usize) -> Self {
        Self(id)
    }
}

pub struct Task {
    pub id: TaskID,
    pub state_isf: InterruptStackFrameValue,
    state_reg: Registers,
}

use lazy_static::lazy_static;
lazy_static! {
    pub static ref TASKMANAGER: Mutex<TaskManager> = Mutex::new(TaskManager::new());
}

pub struct TaskManager {
    current_task: Option<TaskID>,
}
