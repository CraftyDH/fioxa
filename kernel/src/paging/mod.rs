use core::{mem::ManuallyDrop, ops::Deref, ptr};

use conquer_once::spin::Lazy;
use page_allocator::global_allocator;
use page_table_manager::Size4KB;
use x86_64::registers::control::Cr3;

use crate::mutex::Spinlock;

use self::page_table_manager::{Page, PageLvl3, PageLvl4, PageTable};

pub mod offset_map;
pub mod page_allocator;
pub mod page_directory;
pub mod page_mapper;
pub mod page_table_manager;

pub const fn gen_lvl3_map() -> Lazy<Spinlock<PageTable<'static, PageLvl3>>> {
    Lazy::new(|| {
        Spinlock::new(unsafe {
            let page = global_allocator().allocate_page().unwrap();
            PageTable::from_page(page)
        })
    })
}

pub static OFFSET_MAP: Lazy<Spinlock<PageTable<'static, PageLvl3>>> = gen_lvl3_map();
pub static KERNEL_DATA_MAP: Lazy<Spinlock<PageTable<'static, PageLvl3>>> = gen_lvl3_map();

pub static KERNEL_HEAP_MAP: Lazy<Spinlock<PageTable<'static, PageLvl3>>> = gen_lvl3_map();
pub static PER_CPU_MAP: Lazy<Spinlock<PageTable<'static, PageLvl3>>> = gen_lvl3_map();

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

bitflags::bitflags! {
    #[derive(Debug, Clone, Copy)]
    pub struct MemoryMappingFlags: u8 {
        const WRITEABLE  = 1 << 0;
        const USERSPACE  = 1 << 1;
    }
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

pub fn virt_addr_offset_mut<T>(t: *mut T) -> *mut T {
    virt_addr_for_phys(t as u64) as *mut T
}

pub fn phys_addr_for_virt(virt: u64) -> u64 {
    virt.checked_sub(unsafe { MEM_OFFSET })
        .expect("expected virt addr")
}

pub struct AllocatedPage<A: PageAllocator> {
    page: Page<Size4KB>,
    alloc: A,
}

impl<A: PageAllocator> AllocatedPage<A> {
    pub fn new(alloc: A) -> Option<Self> {
        unsafe { alloc.allocate_page().map(|p| Self::from_raw(p, alloc)) }
    }

    pub unsafe fn from_raw(page: Page<Size4KB>, alloc: A) -> Self {
        Self { page, alloc }
    }

    pub fn into_raw(self) -> Page<Size4KB> {
        let mut this = ManuallyDrop::new(self);
        unsafe { ptr::drop_in_place(&mut this.alloc) };
        this.page
    }

    pub fn alloc(&self) -> &A {
        &self.alloc
    }
}

impl<A: PageAllocator> Deref for AllocatedPage<A> {
    type Target = Page<Size4KB>;

    fn deref(&self) -> &Self::Target {
        &self.page
    }
}

impl<A: PageAllocator> Drop for AllocatedPage<A> {
    fn drop(&mut self) {
        unsafe {
            self.alloc.free_page(self.page);
        }
    }
}

pub trait PageAllocator {
    fn allocate_page(&self) -> Option<Page<Size4KB>>;

    fn allocate_pages(&self, count: usize) -> Option<Page<Size4KB>>;

    unsafe fn free_page(&self, page: Page<Size4KB>);

    unsafe fn free_pages(&self, page: Page<Size4KB>, count: usize);
}

pub struct GlobalPageAllocator;

impl PageAllocator for GlobalPageAllocator {
    fn allocate_page(&self) -> Option<Page<Size4KB>> {
        global_allocator().allocate_page()
    }

    fn allocate_pages(&self, count: usize) -> Option<Page<Size4KB>> {
        global_allocator().allocate_pages(count)
    }

    unsafe fn free_page(&self, page: Page<Size4KB>) {
        global_allocator().free_page(page);
    }

    unsafe fn free_pages(&self, page: Page<Size4KB>, count: usize) {
        global_allocator().free_pages(page, count);
    }
}
