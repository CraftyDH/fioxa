use core::ptr::NonNull;

use acpi::{AcpiError, AcpiHandler, AcpiTables, PhysicalMapping};
use x86_64::instructions::interrupts::without_interrupts;

use crate::{
    cpu_localstorage::CPULocalStorageRW,
    paging::{page_mapper::PageMapping, MemoryMappingFlags},
    scheduling::without_context_switch,
};

pub fn prepare_acpi(rsdp: usize) -> Result<AcpiTables<FioxaAcpiHandler>, AcpiError> {
    let root_acpi_handler = unsafe { acpi::AcpiTables::from_rsdp(FioxaAcpiHandler, rsdp) }?;

    println!("ACPI");
    for y in &root_acpi_handler.sdts {
        println!("{}", y.0);
    }
    Ok(root_acpi_handler)
}

#[derive(Clone)]
pub struct FioxaAcpiHandler;

impl AcpiHandler for FioxaAcpiHandler {
    unsafe fn map_physical_region<T>(
        &self,
        physical_address: usize,
        size: usize,
    ) -> acpi::PhysicalMapping<Self, T> {
        let thread = CPULocalStorageRW::get_current_task();

        let base = physical_address & !0xFFF;
        let end = (physical_address + size + 0xFFF) & !0xFFF;
        let mapped_size = end - base;

        without_interrupts(|| {
            let mut mem = thread.process.memory.lock();

            let vaddr_base = mem.page_mapper.insert_mapping(
                PageMapping::new_mmap(base, mapped_size),
                MemoryMappingFlags::WRITEABLE,
            );

            PhysicalMapping::new(
                physical_address,
                NonNull::new((vaddr_base + (physical_address & 0xFFF)) as *mut T).unwrap(),
                size,
                mapped_size,
                self.clone(),
            )
        })
    }

    fn unmap_physical_region<T>(region: &acpi::PhysicalMapping<Self, T>) {
        let thread = CPULocalStorageRW::get_current_task();

        let base = (region.virtual_start().as_ptr() as usize) & !0xFFF;
        without_context_switch(|| {
            let mut mem = thread.process.memory.lock();

            unsafe {
                mem.page_mapper
                    .free_mapping(base..base + region.mapped_length())
                    .unwrap()
            }
        })
    }
}
