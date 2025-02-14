use uefi::boot::{AllocateType, MemoryType, allocate_pages};
use x86_64::registers::control::Cr3;

use self::page_table_manager::PageTableManager;

pub mod page_directory;
pub mod page_map_index;
pub mod page_table_manager;

pub unsafe fn get_uefi_active_mapper() -> PageTableManager {
    let (lv4_table, _) = Cr3::read();

    let phys = lv4_table.start_address();

    PageTableManager::new(phys.as_u64())
}

pub unsafe fn clone_pml4(ptm: &PageTableManager) -> PageTableManager {
    let addr = ptm.get_lvl4_addr();
    let new_page = allocate_pages(AllocateType::AnyPages, MemoryType::LOADER_DATA, 1)
        .unwrap()
        .as_ptr();
    unsafe {
        core::ptr::write_bytes(new_page, 0, 0x1000);
        // Copy first entry
        core::ptr::copy_nonoverlapping(addr as *mut u8, new_page as *mut u8, 0x1000 / 512);
        PageTableManager::new(new_page as u64)
    }
}
