pub mod bitfields;

use core::{arch::x86_64::_mm_pause, mem::transmute, ptr::read_volatile};

use acpi::HpetInfo;
use kernel_sys::types::VMMapFlags;

use crate::paging::{
    KERNEL_LVL4, MemoryLoc,
    page::{Page, Size4KB},
    page_allocator::global_allocator,
    page_table::{MapMemoryError, Mapper},
};

use self::bitfields::CapabilitiesIDRegister;

const FEMPTOSECOND: u64 = 10u64.pow(15);
const MILLISECOND: u64 = 10u64.pow(3);

pub struct HPET {
    pub info: HpetInfo,
    capabilities: CapabilitiesIDRegister,
}

impl HPET {
    pub fn base_addr(&self) -> usize {
        self.info.base_address + MemoryLoc::PhysMapOffset as usize
    }

    pub fn new(hpet: HpetInfo) -> Self {
        let hpet_base_vaddr = MemoryLoc::PhysMapOffset as usize + hpet.base_address;

        // Map into the global mapping
        match KERNEL_LVL4.lock().map(
            global_allocator(),
            Page::<Size4KB>::new(hpet_base_vaddr as u64),
            Page::<Size4KB>::new(hpet.base_address as u64),
            VMMapFlags::WRITEABLE,
        ) {
            Ok(f) => f.flush(),
            Err(MapMemoryError::MemAlreadyMapped { to, current, .. }) if to == current => {
                warn!("HPET override mapping")
            }
            Err(e) => panic!("cannot ident map because {e:?}"),
        }

        let x = unsafe { core::ptr::read_volatile(hpet_base_vaddr as *const u64) };
        let capabilities: CapabilitiesIDRegister = unsafe { transmute(x) };

        // Enable hpet counting
        let x = (hpet_base_vaddr + 0x10) as *mut u64;
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
            read_volatile((self.base_addr() + 0xF0) as *const u64)
                / (FEMPTOSECOND / MILLISECOND / self.capabilities.counter_tick_period() as u64)
        }
    }

    pub fn spin_ms(&self, ms: u64) {
        let end = self.get_uptime() + ms;
        while end > self.get_uptime() {
            _mm_pause();
        }
    }
}
