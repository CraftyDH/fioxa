use core::ptr::NonNull;

use acpi::{AcpiHandler, PhysicalMapping};

use crate::{
    cpu_localstorage::CPULocalStorageRW,
    paging::{MemoryMappingFlags, page_mapper::PageMapping},
};

#[derive(Clone)]
pub struct FioxaAcpiHandler;

impl AcpiHandler for FioxaAcpiHandler {
    unsafe fn map_physical_region<T>(
        &self,
        physical_address: usize,
        size: usize,
    ) -> acpi::PhysicalMapping<Self, T> {
        let thread = unsafe { CPULocalStorageRW::get_current_task() };

        let base = physical_address & !0xFFF;
        let end = (physical_address + size + 0xFFF) & !0xFFF;
        let mapped_size = end - base;

        let mut mem = thread.process().memory.lock();

        let vaddr_base = unsafe {
            mem.page_mapper.insert_mapping_set(
                PageMapping::new_mmap(base, mapped_size),
                MemoryMappingFlags::WRITEABLE,
            )
        };

        unsafe {
            PhysicalMapping::new(
                physical_address,
                NonNull::new((vaddr_base + (physical_address & 0xFFF)) as *mut T).unwrap(),
                size,
                mapped_size,
                self.clone(),
            )
        }
    }

    fn unmap_physical_region<T>(region: &acpi::PhysicalMapping<Self, T>) {
        unsafe {
            let thread = CPULocalStorageRW::get_current_task();

            let base = (region.virtual_start().as_ptr() as usize) & !0xFFF;
            let mut mem = thread.process().memory.lock();

            mem.page_mapper
                .free_mapping(base..base + region.mapped_length())
                .unwrap()
        }
    }
}
