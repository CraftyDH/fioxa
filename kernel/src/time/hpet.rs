pub mod bitfields;

use core::{arch::x86_64::_mm_pause, mem::transmute, ptr::read_volatile};

use acpi::HpetInfo;

use crate::paging::page_table_manager::{ident_map_curr_process, Page, Size4KB};

use self::bitfields::CapabilitiesIDRegister;

const FEMPTOSECOND: u64 = 10u64.pow(15);
const MILLISECOND: u64 = 10u64.pow(3);

pub struct HPET {
    info: HpetInfo,
    capabilities: CapabilitiesIDRegister,
}

impl HPET {
    pub fn new(hpet: HpetInfo) -> Self {
        unsafe { ident_map_curr_process(Page::<Size4KB>::new(hpet.base_address as u64), true) };
        let x = unsafe { core::ptr::read_volatile(hpet.base_address as *const u64) };
        let capabilities: CapabilitiesIDRegister = unsafe { transmute(x) };

        // Enable hpet counting
        let x = (hpet.base_address + 0x10) as *mut u64;
        let v = unsafe { x.read_volatile() };
        unsafe { x.write_volatile(v | 1) }

        Self {
            info: hpet,
            capabilities,
        }
    }

    // Returns system uptime in milliseconds
    pub fn get_uptime(&self) -> u64 {
        unsafe {
            read_volatile((self.info.base_address + 0xF0) as *const u64)
                / (FEMPTOSECOND / MILLISECOND / self.capabilities.counter_tick_period() as u64)
        }
    }

    pub fn spin_ms(&self, ms: u64) {
        let end = self.get_uptime() + ms;
        while end > self.get_uptime() {
            unsafe { _mm_pause() };
        }
    }
}
