use core::ptr::NonNull;

use acpi::{AcpiError, AcpiHandler, AcpiTables, PhysicalMapping};

use crate::paging::get_uefi_active_mapper;

pub fn prepare_acpi(rsdp: usize) -> Result<AcpiTables<FioxaAcpiHandler>, AcpiError> {
    // let handler = FioxaAcpiHandler::new(frame_allocator);
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

        mapper
            .map_memory(physical_address as u64, physical_address as u64)
            .unwrap()
            .flush();

        PhysicalMapping::new(
            physical_address,
            NonNull::new(physical_address as *mut T).unwrap(),
            size,
            size,
            self.clone(),
        )
    }

    fn unmap_physical_region<T>(region: &acpi::PhysicalMapping<Self, T>) {
        let mapper = unsafe { get_uefi_active_mapper() };

        mapper
            .unmap_memory(region.virtual_start().as_ptr() as u64)
            .unwrap()
            .flush();
    }
}
