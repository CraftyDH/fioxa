use core::ptr::NonNull;

use acpi::{AcpiHandler, PhysicalMapping};
use alloc::sync::Arc;
use kernel_sys::types::VMMapFlags;

use crate::{cpu_localstorage::CPULocalStorageRW, mutex::Spinlock, vm::VMO};

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
            mem.region
                .map_vmo(
                    Arc::new(Spinlock::new(VMO::new_mmap(base, mapped_size))),
                    VMMapFlags::WRITEABLE,
                    None,
                )
                .unwrap()
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

            mem.region.unmap(base, region.mapped_length()).unwrap()
        }
    }
}
