pub mod uefi;

use x86_64::structures::paging::OffsetPageTable;
use x86_64::{registers::control::Cr3, structures::paging::PageTable, VirtAddr};

pub unsafe fn get_active_lvl4_table(physical_memory_offset: VirtAddr) -> &'static mut PageTable {
    let (lv4_table, _) = Cr3::read();

    let phys = lv4_table.start_address();
    let virt = physical_memory_offset + phys.as_u64();
    let page_table_ptr: *mut PageTable = virt.as_mut_ptr();

    &mut *page_table_ptr
}

pub unsafe fn get_active_mapper(physical_memory_offset: VirtAddr) -> OffsetPageTable<'static> {
    let lvl4_table = get_active_lvl4_table(physical_memory_offset);
    OffsetPageTable::new(lvl4_table, physical_memory_offset)
}

/// UEFI and our kernel identity maps pysical memory at offset 0x0
/// therefore we don't need to know to vaddr start
pub unsafe fn get_uefi_active_mapper() -> OffsetPageTable<'static> {
    get_active_mapper(VirtAddr::new(0))
}
