use core::cmp::Reverse;

use acpi::AcpiTables;
use alloc::{collections::binary_heap::BinaryHeap, sync::Arc};
use spin::Once;

use crate::{acpi::FioxaAcpiHandler, mutex::Spinlock, scheduling::process::Thread};

pub mod hpet;

pub static HPET: Once<hpet::HPET> = Once::new();

pub fn spin_sleep_ms(time: u64) {
    HPET.get().unwrap().spin_ms(time)
}

pub fn init_time(acpi_tables: &AcpiTables<FioxaAcpiHandler>) {
    // PIC.lock().set_divisor(10000);
    if let Ok(hpet_info) = acpi::HpetInfo::new(acpi_tables) {
        HPET.call_once(|| hpet::HPET::new(hpet_info));
    };
}

pub fn uptime() -> u64 {
    HPET.get().unwrap().get_uptime()
}

#[derive(Debug)]
pub struct SleptProcess {
    pub wakeup: u64,
    pub thread: Arc<Thread>,
}

impl PartialEq for SleptProcess {
    fn eq(&self, other: &Self) -> bool {
        self.wakeup == other.wakeup && Arc::ptr_eq(&self.thread, &other.thread)
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
pub static SLEPT_PROCESSES: Spinlock<BinaryHeap<Reverse<SleptProcess>>> =
    Spinlock::new(BinaryHeap::new());

pub fn check_sleep() {
    let uptime = HPET.get().unwrap().get_uptime();

    // if over the target, try waking up processes
    if let Some(mut procs) = SLEPT_PROCESSES.try_lock() {
        // pop elements from the heap if they should be woken up
        while procs.peek().is_some_and(|p| p.0.wakeup <= uptime) {
            procs.pop().unwrap().0.thread.wake();
        }
    }
}
