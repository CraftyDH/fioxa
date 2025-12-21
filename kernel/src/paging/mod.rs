use core::{mem::ManuallyDrop, ops::Deref, ptr};

use page::Size4KB;
use page_allocator::global_allocator;
use page_table::{TableLevel3, TableLevel4};
use spin::Lazy;

use crate::{
    mutex::Spinlock,
    paging::page_table::{PageTableOwned, PageTableStatic},
};

use self::page::Page;

pub mod offset_map;
pub mod page;
pub mod page_allocator;
pub mod page_directory;
pub mod page_table;

/// KERNEL map for context 0 / scheduler
pub static KERNEL_LVL4: Lazy<Spinlock<PageTableStatic<TableLevel4>>> = Lazy::new(|| {
    Spinlock::new(
        PageTableOwned::new_with_global(GlobalPageAllocator)
            .unwrap()
            .leak(),
    )
});
pub static OFFSET_MAP: Lazy<Spinlock<PageTableStatic<TableLevel3>>> =
    Lazy::new(|| Spinlock::new(PageTableOwned::new(GlobalPageAllocator).unwrap().leak()));
pub static KERNEL_DATA_MAP: Lazy<Spinlock<PageTableStatic<TableLevel3>>> =
    Lazy::new(|| Spinlock::new(PageTableOwned::new(GlobalPageAllocator).unwrap().leak()));
pub static KERNEL_STACKS_MAP: Lazy<Spinlock<PageTableStatic<TableLevel3>>> =
    Lazy::new(|| Spinlock::new(PageTableOwned::new(GlobalPageAllocator).unwrap().leak()));
pub static KERNEL_HEAP_MAP: Lazy<Spinlock<PageTableStatic<TableLevel3>>> =
    Lazy::new(|| Spinlock::new(PageTableOwned::new(GlobalPageAllocator).unwrap().leak()));
pub static PER_CPU_MAP: Lazy<Spinlock<PageTableStatic<TableLevel3>>> =
    Lazy::new(|| Spinlock::new(PageTableOwned::new(GlobalPageAllocator).unwrap().leak()));

pub type MemoryLoc = MemoryLoc64bit48bits;

/// First 4 bits need to either be 0xFFFF or 0x0000
/// (depending on which side the 48th bit is, 0..7 = 0x0000, 8..F = 0xFFFF)
#[repr(u64)]
#[allow(clippy::mixed_case_hex_literals)]
pub enum MemoryLoc64bit48bits {
    EndUserMem = 0x0000_7FFFFFFFFFFF,
    GlobalMapping = 0xffff_A00000000000,
    /// Each cpu core is given 0x10_0000 (1mb) virtual memory
    PerCpuMem = 0xffff_AC0000000000,
    PhysMapOffset = 0xffff_E00000000000,     // 448 (16tb)
    _EndPhysMapOffset = 0xffff_EFFFFFFFFFFF, // <450 (10tb)
    KernelStacks = 0xffff_FE8000000000,      // 509
    KernelHeap = 0xffff_FF0000000000,        // 510
    KernelStart = 0xffff_FF8000000000,       // 511
}

static mut MEM_OFFSET: u64 = 0;
pub unsafe fn set_mem_offset(n: u64) -> u64 {
    unsafe { core::ptr::replace(&raw mut MEM_OFFSET, n) }
}

pub fn get_mem_offset() -> u64 {
    unsafe { MEM_OFFSET }
}

#[track_caller]
pub fn virt_addr_for_phys(phys: u64) -> u64 {
    assert_ne!(phys, 0);
    phys.checked_add(unsafe { MEM_OFFSET })
        .expect("expected phys addr")
}

#[track_caller]
pub fn virt_addr_offset<T>(t: *const T) -> *const T {
    virt_addr_for_phys(t as u64) as *const T
}

#[track_caller]
pub fn virt_addr_offset_mut<T>(t: *mut T) -> *mut T {
    virt_addr_for_phys(t as u64) as *mut T
}

#[track_caller]
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

    pub fn map_alloc<B: PageAllocator>(self, alloc: B) -> AllocatedPage<B> {
        AllocatedPage {
            page: self.into_raw(),
            alloc,
        }
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

impl<A: PageAllocator> PageAllocator for &A {
    fn allocate_page(&self) -> Option<Page<Size4KB>> {
        (*self).allocate_page()
    }

    fn allocate_pages(&self, count: usize) -> Option<Page<Size4KB>> {
        (*self).allocate_pages(count)
    }

    unsafe fn free_page(&self, page: Page<Size4KB>) {
        unsafe { (*self).free_page(page) };
    }

    unsafe fn free_pages(&self, page: Page<Size4KB>, count: usize) {
        unsafe { (*self).free_pages(page, count) };
    }
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
        unsafe { global_allocator().free_page(page) };
    }

    unsafe fn free_pages(&self, page: Page<Size4KB>, count: usize) {
        unsafe { global_allocator().free_pages(page, count) };
    }
}
