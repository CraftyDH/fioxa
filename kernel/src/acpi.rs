use core::ptr::NonNull;

use acpi::{AcpiError, AcpiHandler, AcpiTables, PhysicalMapping};

use crate::paging::{
    get_uefi_active_mapper,
    page_table_manager::{ident_map_curr_process, page_4kb, Mapper},
};

pub fn prepare_acpi(rsdp: usize) -> Result<AcpiTables<FioxaAcpiHandler>, AcpiError> {
    // let handler = FioxaAcpiHandler::new(frame_allocator);
    ident_map_curr_process((rsdp as u64) & !0xFFF, false);

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

        for page in (physical_address & !0xFFF..(physical_address + size + 0xFFF)).step_by(0x1000) {
            mapper
                .map_memory(page_4kb(page as u64), page_4kb(page as u64))
                .unwrap()
                .flush();
        }

        PhysicalMapping::new(
            physical_address & !0xFFF,
            NonNull::new(physical_address as *mut T).unwrap(),
            size,
            (physical_address + size + 0xFFF) & !0xFFF - physical_address & !0xFFF,
            self.clone(),
        )
    }

    fn unmap_physical_region<T>(region: &acpi::PhysicalMapping<Self, T>) {
        let mut mapper = unsafe { get_uefi_active_mapper() };

        for page in (region.physical_start()..(region.physical_start() + region.mapped_length()))
            .step_by(0x1000)
        {
            //todo fix
            // mapper.unmap_memory(page_4kb(page as u64)).unwrap().flush();
        }
    }
}
