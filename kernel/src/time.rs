use acpi::AcpiTables;
use spin::Mutex;

use crate::acpi::FioxaAcpiHandler;

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
