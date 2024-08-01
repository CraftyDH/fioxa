use acpi::AcpiTables;
use alloc::{collections::BTreeMap, sync::Weak, vec::Vec};
use conquer_once::spin::{Lazy, OnceCell};
use spin::Mutex;

use crate::{
    acpi::FioxaAcpiHandler,
    scheduling::{
        process::{ThreadHandle, ThreadStatus},
        taskmanager::push_task_queue,
    },
};

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

pub static SLEPT_PROCESSES: Lazy<Mutex<BTreeMap<u64, Vec<Weak<ThreadHandle>>>>> =
    Lazy::new(|| Mutex::new(BTreeMap::new()));

pub fn check_sleep(uptime: u64) {
    // if over the target, try waking up processes
    if let Some(mut procs) = SLEPT_PROCESSES.try_lock() {
        // check if any elements could be taken
        if procs.first_entry().is_none_or(|e| *e.key() > uptime) {
            return;
        }

        procs
            .extract_if(|&req_time, _| req_time <= uptime)
            .flat_map(|(_, handles)| handles)
            .filter_map(|handle| handle.upgrade())
            .for_each(|handle| {
                let status = &mut *handle.status.lock();
                match core::mem::take(status) {
                    ThreadStatus::Ok | ThreadStatus::BlockingRet(_) => {
                        panic!("bad state")
                    }
                    ThreadStatus::Blocking => *status = ThreadStatus::BlockingRet(0),
                    ThreadStatus::Blocked(thread) => push_task_queue(thread),
                }
            });
    }
}
