use core::cmp::Reverse;

use acpi::AcpiTables;
use alloc::{collections::binary_heap::BinaryHeap, sync::Weak};
use conquer_once::spin::OnceCell;
use spin::Mutex;

use crate::{acpi::FioxaAcpiHandler, scheduling::process::ThreadHandle};

pub mod hpet;
pub mod pit;

pub static HPET: OnceCell<hpet::HPET> = OnceCell::uninit();

pub fn spin_sleep_ms(time: u64) {
    HPET.get().unwrap().spin_ms(time)
}

pub fn init_time(acpi_tables: &AcpiTables<FioxaAcpiHandler>) {
    // PIC.lock().set_divisor(10000);
    if let Ok(hpet_info) = acpi::HpetInfo::new(acpi_tables) {
        HPET.init_once(|| hpet::HPET::new(hpet_info));
    };
}

pub fn uptime() -> u64 {
    HPET.get().unwrap().get_uptime()
}

#[derive(Debug)]
pub struct SleptProcess {
    pub wakeup: u64,
    pub thread: Weak<ThreadHandle>,
}

impl PartialEq for SleptProcess {
    fn eq(&self, other: &Self) -> bool {
        self.wakeup == other.wakeup && self.thread.ptr_eq(&other.thread)
    }
}

impl Eq for SleptProcess {}

impl PartialOrd for SleptProcess {
    fn partial_cmp(&self, other: &Self) -> Option<core::cmp::Ordering> {
        self.wakeup.partial_cmp(&other.wakeup)
    }
}

impl Ord for SleptProcess {
    fn cmp(&self, other: &Self) -> core::cmp::Ordering {
        self.wakeup.cmp(&other.wakeup)
    }
}

/// We want a min-heap not max-heap, so reverse the ordering
pub static SLEPT_PROCESSES: Mutex<BinaryHeap<Reverse<SleptProcess>>> =
    Mutex::new(BinaryHeap::new());

pub fn check_sleep(uptime: u64) {
    // if over the target, try waking up processes
    if let Some(mut procs) = SLEPT_PROCESSES.try_lock() {
        // pop elements from the heap if they should be woken up
        while procs.peek().is_some_and(|p| p.0.wakeup <= uptime) {
            if let Some(p) = procs.pop().unwrap().0.thread.upgrade() {
                p.wake_up();
            }
        }
    }
}
