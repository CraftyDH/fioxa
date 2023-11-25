use conquer_once::spin::Lazy;
use spin::Mutex;
use x86_64::registers::control::Cr3;

use self::{
    page_allocator::request_page_early,
    page_table_manager::{Page, PageLvl3, PageLvl4, PageTable},
};

pub mod offset_map;
pub mod page_allocator;
pub mod page_directory;
pub mod page_mapper;
pub mod page_table_manager;

pub const fn gen_lvl3_map() -> Lazy<Mutex<PageTable<'static, PageLvl3>>> {
    Lazy::new(|| {
        Mutex::new(unsafe {
            // The AP startup code needs a 32 bit ptr
            let page = request_page_early().unwrap();
            assert!(page.get_address() <= u32::MAX as u64);
            PageTable::from_page(page)
        })
    })
}

pub static OFFSET_MAP: Lazy<Mutex<PageTable<'static, PageLvl3>>> = gen_lvl3_map();
pub static KERNEL_DATA_MAP: Lazy<Mutex<PageTable<'static, PageLvl3>>> = gen_lvl3_map();

pub static KERNEL_HEAP_MAP: Lazy<Mutex<PageTable<'static, PageLvl3>>> = gen_lvl3_map();
pub static PER_CPU_MAP: Lazy<Mutex<PageTable<'static, PageLvl3>>> = gen_lvl3_map();

pub unsafe fn get_uefi_active_mapper() -> PageTable<'static, PageLvl4> {
    let (lv4_table, _) = Cr3::read();

    let phys = lv4_table.start_address();

    PageTable::from_page(Page::new(phys.as_u64()))
}

pub type MemoryLoc = MemoryLoc64bit48bits;

/// First 4 bits need to either be 0xFFFF or 0x0000
/// (depending on which side the 48th bit is, 0..7 = 0x0000, 8..F = 0xFFFF)
#[repr(u64)]
#[allow(clippy::mixed_case_hex_literals)]
pub enum MemoryLoc64bit48bits {
    EndUserMem = 0x0000_FFFFFFFFFFFF,
    GlobalMapping = 0xffff_A00000000000,
    /// Each cpu core is given 0x10_0000 (1mb) virtual memory
    PerCpuMem = 0xffff_AC0000000000,
    PhysMapOffset = 0xffff_E00000000000,     // 448 (16tb)
    _EndPhysMapOffset = 0xffff_EFFFFFFFFFFF, // <450 (10tb)
    KernelHeap = 0xffff_FF0000000000,        // 510
    KernelStart = 0xffff_FF8000000000,       // 511
}

static mut MEM_OFFSET: u64 = 0;
pub unsafe fn set_mem_offset(n: u64) {
    unsafe { MEM_OFFSET = n }
}

pub fn virt_addr_for_phys(phys: u64) -> u64 {
    phys.checked_add(unsafe { MEM_OFFSET })
        .expect("expected phys addr")
}

pub fn virt_addr_offset<T>(t: *const T) -> *const T {
    virt_addr_for_phys(t as u64) as *const T
}

pub fn phys_addr_for_virt(virt: u64) -> u64 {
    virt.checked_sub(unsafe { MEM_OFFSET })
        .expect("expected virt addr")
}
