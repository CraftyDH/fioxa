use core::sync::atomic::AtomicU64;

use acpi::AcpiTables;
use alloc::collections::BTreeMap;
use conquer_once::spin::Lazy;
use spin::Mutex;

use crate::{
    acpi::FioxaAcpiHandler,
    scheduling::{
        process::LinkedThreadList, taskmanager::append_task_queue, without_context_switch,
    },
    syscall::INTERNAL_KERNEL_WAKER_SLEEPD,
    thread_waker::{atomic_waker_loop, AtomicThreadWaker},
};

use self::pit::ProgrammableIntervalTimer;

pub mod hpet;
pub mod pit;

static PIC: Mutex<ProgrammableIntervalTimer> = Mutex::new(ProgrammableIntervalTimer::new());

static HPET: Mutex<Option<hpet::HPET>> = Mutex::new(None);

pub fn spin_sleep_ms(time: u64) {
    // Use HPET if possible otherwise PIC
    match &*HPET.lock() {
        Some(hpet) => hpet.spin_ms(time),
        None => PIC.lock().spin_ms(time),
    };
}

pub fn init_time(acpi_tables: &AcpiTables<FioxaAcpiHandler>) {
    PIC.lock().set_divisor(10000);
    if let Ok(hpet_info) = acpi::HpetInfo::new(acpi_tables) {
        let dev = hpet::HPET::new(hpet_info);
        let h = &mut *HPET.lock();
        *h = Some(dev);
    };
}

pub fn uptime() -> u64 {
    match &*HPET.lock() {
        Some(hpet) => hpet.get_uptime(),
        None => pit::get_uptime(),
    }
}

pub static SLEEP_WAKER: AtomicThreadWaker = AtomicThreadWaker::new();
pub static SLEEP_TARGET: AtomicU64 = AtomicU64::new(u64::MAX);
pub static SLEPT_PROCESSES: Lazy<Mutex<BTreeMap<u64, LinkedThreadList>>> =
    Lazy::new(|| Mutex::new(BTreeMap::new()));

pub fn sleepd() {
    atomic_waker_loop(&SLEEP_WAKER, INTERNAL_KERNEL_WAKER_SLEEPD, || {
        without_context_switch(|| {
            let time = pit::get_uptime();
            let mut procs = SLEPT_PROCESSES.lock();
            procs.retain(|&req_time, el| {
                if req_time <= time {
                    append_task_queue(el);
                    false
                } else {
                    true
                }
            });
            SLEEP_TARGET.store(
                procs.first_key_value().map(|(&k, _)| k).unwrap_or(u64::MAX),
                core::sync::atomic::Ordering::Relaxed,
            )
        });
    });
}
