use core::ptr::NonNull;

use acpi::{AcpiError, AcpiHandler, AcpiTables, PhysicalMapping};

use x86_64::{
    structures::paging::{
        mapper::{MapToError, UnmapError},
        Mapper, Page, PageTableFlags, PhysFrame, Size4KiB,
    },
    PhysAddr, VirtAddr,
};

use crate::memory::{get_active_mapper, uefi::FRAME_ALLOCATOR};

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
        let mut mapper = get_active_mapper(VirtAddr::from_ptr(0 as *const u8));

        let res = mapper.identity_map(
            PhysFrame::<Size4KiB>::containing_address(PhysAddr::new(physical_address as u64)),
            PageTableFlags::WRITABLE | PageTableFlags::PRESENT,
            &mut *FRAME_ALLOCATOR.lock(),
        );

        if let Err(e) = res {
            match e {
                MapToError::FrameAllocationFailed => panic!("{:?}", e),
                // Doesn't matter, we are identity mapped
                MapToError::ParentEntryHugePage => {}
                MapToError::PageAlreadyMapped(a) => println!("Allready Mapped: {:?}", a),
            }
        } else {
            res.unwrap().flush();
        }

        PhysicalMapping::new(
            physical_address,
            NonNull::new(physical_address as *mut T).unwrap(),
            size,
            size,
            self.clone(),
        )
    }

    fn unmap_physical_region<T>(region: &acpi::PhysicalMapping<Self, T>) {
        println!("Unmap");
        let mut mapper = unsafe { get_active_mapper(VirtAddr::from_ptr(0 as *const u8)) };

        let res = mapper.unmap(Page::<Size4KiB>::containing_address(VirtAddr::new(
            region.virtual_start().as_ptr() as u64,
        )));

        if let Err(e) = res {
            match e {
                UnmapError::InvalidFrameAddress(a) => panic!("Invalid Frame addr: {:?}", a),
                UnmapError::ParentEntryHugePage => println!("Parent Mapped"),
                UnmapError::PageNotMapped => println!("Not Mapped"),
            }
        } else {
            res.unwrap().1.flush();
        }
    }
}
