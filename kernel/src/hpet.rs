use core::{arch::x86_64::_mm_pause, mem::transmute, ptr::read_volatile};

use acpi::AcpiTables;
use modular_bitfield::{bitfield, specifiers::B5};

const FEMPTOSECOND: u64 = 10u64.pow(15);
const MILLISECOND: u64 = 10u64.pow(3);

// Pointer to the main counter of the HPET
pub static mut UPTIME: *mut u64 = 0 as *mut u64;
// Period the counter increments in FEMPTOSECONDS
pub static mut COUNTER_TICK_PERIOD: u64 = 1;
use crate::{acpi::FioxaAcpiHandler, paging::get_uefi_active_mapper, syscall::yield_now};

// Returns system uptime in milliseconds
pub fn get_uptime() -> u64 {
    // get_uptime_fempto() * MILLISECOND
    unsafe { read_volatile(UPTIME) / (FEMPTOSECOND / MILLISECOND / COUNTER_TICK_PERIOD) }
}

pub fn spin_sleep_ms(time: u64) {
    let end = get_uptime() + time;
    while end > get_uptime() {
        unsafe { _mm_pause() };
    }
}

//* Do not call if not in a thread
pub fn sleep_ms(time: u64) {
    let end = get_uptime() + time;
    while end > get_uptime() {
        yield_now();
        unsafe { _mm_pause() };
    }
}

pub fn init_hpet(acpi_tables: &AcpiTables<FioxaAcpiHandler>) {
    let hpet = acpi::HpetInfo::new(acpi_tables).unwrap();
    let mut mapper = unsafe { get_uefi_active_mapper() };

    mapper
        .map_memory(hpet.base_address as u64, hpet.base_address as u64, true)
        .unwrap()
        .flush();
    let x = unsafe { core::ptr::read_volatile(hpet.base_address as *const u64) };
    let y: CapabilitiesIDRegister = unsafe { transmute(x) };

    unsafe { COUNTER_TICK_PERIOD = y.counter_tick_period() as u64 }

    // Enable hpet counting
    let x = (hpet.base_address + 0x10) as *mut u64;
    let v = unsafe { x.read_volatile() };
    unsafe { x.write_volatile(v | 1) }

    unsafe { UPTIME = (hpet.base_address + 0xF0) as *mut u64 };
}

#[bitfield]
#[derive(Debug)]
struct CapabilitiesIDRegister {
    rev_id: u8,
    timer_cnt: B5,
    can_64_bit: bool,
    #[skip]
    _resv: bool,
    legacy_replacement_cap: bool,
    vendor_id: u16,
    counter_tick_period: u32,
}
