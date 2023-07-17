use core::ptr::NonNull;

use acpi::{AcpiError, AcpiHandler, AcpiTables, PhysicalMapping};

use crate::paging::{
    get_uefi_active_mapper,
    page_table_manager::{ident_map_curr_process, Mapper, Page, Size4KB},
};

pub fn prepare_acpi(rsdp: usize) -> Result<AcpiTables<FioxaAcpiHandler>, AcpiError> {
    ident_map_curr_process(Page::<Size4KB>::containing(rsdp as u64), false);

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
        let mut mapper = get_uefi_active_mapper();

        let start = physical_address & !0xFFF;
        let end = (physical_address + size + 0xFFF) & !0xFFF;

        for page in (start..end).step_by(0x1000) {
            mapper
                .identity_map_memory(Page::<Size4KB>::new(page as u64))
                .unwrap()
                .flush();
        }

        PhysicalMapping::new(
            start,
            NonNull::new(physical_address as *mut T).unwrap(),
            size,
            end - start,
            self.clone(),
        )
    }

    fn unmap_physical_region<T>(region: &acpi::PhysicalMapping<Self, T>) {
        let mut mapper = unsafe { get_uefi_active_mapper() };

        for page in (region.physical_start()..(region.physical_start() + region.mapped_length()))
            .step_by(0x1000)
        {
            mapper
                .unmap_memory(Page::<Size4KB>::new(page as u64))
                .unwrap()
                .flush();
        }
    }
}
