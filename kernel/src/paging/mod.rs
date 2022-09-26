use x86_64::registers::control::Cr3;

use self::page_table_manager::PageTableManager;

pub mod page_allocator;
pub mod page_directory;
pub mod page_map_index;
pub mod page_table_manager;
pub mod identity_map;

pub unsafe fn get_uefi_active_mapper() -> PageTableManager {
    let (lv4_table, _) = Cr3::read();

    let phys = lv4_table.start_address();

    PageTableManager::new(phys.as_u64())
}
