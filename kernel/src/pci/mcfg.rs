use acpi::{
    sdt::{SdtHeader, Signature},
    AcpiError, AcpiTable, PhysicalMapping,
};
use alloc::slice;

use crate::acpi::FioxaAcpiHandler;

pub fn get_mcfg(
    acpi_tables: &acpi::AcpiTables<FioxaAcpiHandler>,
) -> Result<PhysicalMapping<FioxaAcpiHandler, MCFG>, AcpiError> {
    let mcfg = unsafe {
        acpi_tables
            .get_sdt::<MCFG>(Signature::MCFG)
            .and_then(|table| table.ok_or(AcpiError::TableMissing(Signature::MCFG)))
    };
    mcfg
}

#[repr(C, packed)]
pub struct MCFG {
    header: SdtHeader,
    _reserved: u64,
}

impl AcpiTable for MCFG {
    fn header(&self) -> &acpi::sdt::SdtHeader {
        &self.header
    }
}

impl MCFG {
    pub fn entries(&self) -> &[MCFGEntry] {
        let length = self.header.length as usize - core::mem::size_of::<MCFG>();

        let number_of_entries = length / core::mem::size_of::<MCFGEntry>();

        unsafe {
            let ptr = (self as *const MCFG as *const u8).add(core::mem::size_of::<MCFG>())
                as *const MCFGEntry;
            slice::from_raw_parts(ptr, number_of_entries)
        }
    }
}

#[repr(C, packed)]
pub struct MCFGEntry {
    pub base_address: u64,
    pub pci_segment_group: u16,
    pub bus_number_start: u8,
    pub bus_number_end: u8,
    _reserved: u32,
}
