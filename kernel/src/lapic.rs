use crate::paging::page_table_manager::{Mapper, Page, PageLvl4, PageTable, Size4KB};

// Local APIC
/// Do not use before this has been initialized in enable_apic
pub const LAPIC_ADDR: u64 = 0xfee00000;

pub fn map_lapic(mapper: &mut PageTable<PageLvl4>) {
    mapper
        .identity_map_memory(Page::<Size4KB>::new(0xfee00000))
        .unwrap()
        .flush();
}

pub fn enable_localapic() {
    let mut val = unsafe { *((LAPIC_ADDR + 0xF0) as *const u32) };
    // Enable
    val |= 1 << 8;
    // Spurious vector
    val |= 0xFF;

    println!("LAPIC ID {:?}", unsafe {
        *((LAPIC_ADDR + 0x20) as *const u32)
    });

    unsafe { *((LAPIC_ADDR + 0xF0) as *mut u32) = val };
}
