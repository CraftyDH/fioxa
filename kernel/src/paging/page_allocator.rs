use bootloader::uefi::boot::MemoryType;
use conquer_once::spin::OnceCell;

use crate::{
    memory::{MemoryMapIter, RESERVED_32BIT_MEM_PAGES},
    mutex::Spinlock,
};

use super::{
    PageAllocator,
    page::{Page, Size4KB},
    virt_addr_for_phys, virt_addr_offset_mut,
};

static GLOBAL_FRAME_ALLOCATOR: OnceCell<Spinlock<PageFrameAllocator>> = OnceCell::uninit();

pub fn frame_alloc_exec<T, F>(closure: F) -> T
where
    F: Fn(&mut PageFrameAllocator) -> T,
{
    closure(&mut GLOBAL_FRAME_ALLOCATOR.get().unwrap().lock())
}

pub fn global_allocator() -> &'static impl PageAllocator {
    GLOBAL_FRAME_ALLOCATOR.get().unwrap()
}

pub unsafe fn init(mmap: MemoryMapIter) {
    let alloc = unsafe { PageFrameAllocator::new(mmap.clone()).into() };
    GLOBAL_FRAME_ALLOCATOR.init_once(|| alloc)
}

pub struct AllocatedPageOrder {
    order: usize,
    base: usize,
}

impl AllocatedPageOrder {
    pub fn base(&self) -> usize {
        self.base
    }
}

pub struct PageMetadata {
    order: usize,
    // These fields only matter if it is the left node
    prev_node: Option<*mut PageMetadata>,
    next_node: Option<*mut PageMetadata>,
}

pub struct PageMetadata32 {
    next_node: Option<*mut PageMetadata32>,
}

// This counts a 1gb zone
const MAX_ORDER: usize = 18;

/// TODO: Implement a bitmap to determine when we can coalese blocks back together

pub struct PageFrameAllocator {
    free_lists: [Option<*mut PageMetadata>; MAX_ORDER + 1],
    reserved_32bit: Option<*mut PageMetadata32>,

    // we reserve 0x8000 specifically for the purpose of booting AP's
    captured_0x8000: bool,
    total_free: usize,
}

unsafe impl Send for PageFrameAllocator {}

#[inline(always)]
pub fn pages_in_order(order: usize) -> usize {
    1 << order
}

impl PageFrameAllocator {
    pub unsafe fn new(mmap: MemoryMapIter) -> Self {
        let mut this = Self {
            free_lists: Default::default(),
            captured_0x8000: false,
            reserved_32bit: None,
            total_free: 0,
        };

        let mut free = mmap
            .map(|e| unsafe { &*e })
            .filter(|e| e.ty == MemoryType::CONVENTIONAL);

        // Capture the reserved pages
        let mut free_found = 0;
        loop {
            let mut entry = *free.by_ref().next().unwrap();

            // ignore page starting at paddr 0
            if entry.phys_start == 0 {
                entry.phys_start = 0x1000;
                entry.page_count -= 1;
            }

            let range = entry.phys_start as usize
                ..(entry.phys_start + (entry.page_count * 0x1000)) as usize;

            let pages = if range.contains(&0x8000) {
                this.captured_0x8000 = true;
                entry.page_count as usize - 1
            } else {
                entry.page_count as usize
            };

            let taken_amount = core::cmp::min(pages, RESERVED_32BIT_MEM_PAGES - free_found);
            free_found += taken_amount;

            // Add the pages to the reserved region
            range
                .step_by(0x1000)
                .take(taken_amount)
                .filter(|&p| p != 0x8000)
                .for_each(|p| unsafe { this.free_32bit_reserved_page(p) });

            if free_found == RESERVED_32BIT_MEM_PAGES {
                let amount_left = entry.page_count as usize - taken_amount;
                this.total_free += amount_left;
                unsafe {
                    this.insert_free_of_range(
                        entry.phys_start as usize + taken_amount * 0x1000,
                        amount_left,
                    );
                }
                break;
            } else if free_found > RESERVED_32BIT_MEM_PAGES {
                unreachable!("logic error")
            }
        }

        for entry in free {
            let start_addr = entry.phys_start as usize;
            let pages_left = entry.page_count as usize;

            this.total_free += pages_left;
            unsafe { this.insert_free_of_range(start_addr, pages_left) };
        }
        this
    }

    pub fn captured_0x8000(&self) -> bool {
        self.captured_0x8000
    }

    // This is safe to call with zero pages
    unsafe fn insert_free_of_range(&mut self, mut start_addr: usize, mut pages_left: usize) {
        while pages_left > 0 {
            // Find the largest order that we can use
            let pages_order = pages_left.ilog2() as usize;
            let address_order = (start_addr / 0x1000).ilog2() as usize;
            let order = core::cmp::min(core::cmp::min(pages_order, address_order), MAX_ORDER);

            unsafe { self.insert_free_of_order(start_addr, order) };

            let page_count = pages_in_order(order);
            pages_left -= page_count;
            start_addr += page_count * 0x1000;
        }
    }

    unsafe fn insert_free_of_order(&mut self, base: usize, order: usize) {
        let left = unsafe { &mut *virt_addr_offset_mut(base as *mut PageMetadata) };
        left.order = order;
        left.next_node = None;
        left.prev_node = None;

        // if order is 0, we only have 1 page
        if order > 0 {
            let last_page_addr_offset = (pages_in_order(order) - 1) * 0x1000;
            let right = unsafe {
                &mut *virt_addr_offset_mut((base + last_page_addr_offset) as *mut PageMetadata)
            };
            right.order = order;
        }

        left.next_node = self.free_lists[order].take();
        self.free_lists[order] = Some(base as *mut PageMetadata);
    }

    // currently splits the left
    pub fn request_page_of_order(&mut self, order: usize) -> Option<AllocatedPageOrder> {
        if order > MAX_ORDER {
            return None;
        }

        let base = if let Some(block) = self.free_lists[order] {
            let b = unsafe { &mut *virt_addr_offset_mut(block) };

            if let Some(nxt) = b.next_node {
                let nxt = unsafe { &mut *virt_addr_offset_mut(nxt) };
                nxt.prev_node = None;
            }

            self.free_lists[order] = b.next_node;
            block as usize
        } else {
            // Request a larger block and split it
            let large_block = self.request_page_of_order(order + 1)?;
            unsafe {
                self.insert_free_of_range(
                    large_block.base + pages_in_order(order) * 0x1000,
                    pages_in_order(order + 1) - pages_in_order(order),
                )
            };
            large_block.base
        };

        Some(AllocatedPageOrder { order, base })
    }

    pub unsafe fn free_32bit_reserved_page(&mut self, page: usize) {
        let meta = unsafe { &mut *virt_addr_offset_mut(page as *mut PageMetadata32) };

        meta.next_node = self.reserved_32bit.take();
        self.reserved_32bit = Some(page as *mut PageMetadata32);
    }

    pub fn free_page_of_order(&mut self, pages: AllocatedPageOrder) {
        unsafe { self.insert_free_of_order(pages.base, pages.order) }
    }

    pub unsafe fn free_page(&mut self, page: Page<Size4KB>) {
        unsafe { self.insert_free_of_order(page.get_address() as usize, 0) }
    }

    pub fn allocate_page(&mut self) -> Option<Page<Size4KB>> {
        let base = self.request_page_of_order(0)?.base as u64;

        unsafe { core::ptr::write_bytes(virt_addr_for_phys(base) as *mut u8, 0, 0x1000) };

        Some(Page::new(base))
    }

    pub fn allocate_page_32bit(&mut self) -> Option<Page<Size4KB>> {
        let block = self.reserved_32bit?;
        let b = unsafe { &mut *virt_addr_offset_mut(block) };
        let base = block as *const _ as u64;
        self.reserved_32bit = b.next_node;
        unsafe { core::ptr::write_bytes(virt_addr_for_phys(base) as *mut u8, 0, 0x1000) };
        Some(Page::new(base))
    }

    pub fn allocate_pages(&mut self, count: usize) -> Option<Page<Size4KB>> {
        // Returns the log 2 rounded down
        let order = count.ilog2() as usize;

        let base = if pages_in_order(order) == count {
            // The count is a block size
            self.request_page_of_order(order)?.base
        } else {
            // Split a larger block
            let large_block = self.request_page_of_order(order + 1)?;

            unsafe {
                self.insert_free_of_range(
                    large_block.base + count * 0x1000,
                    pages_in_order(order + 1) - count,
                )
            }

            large_block.base
        };
        unsafe {
            core::ptr::write_bytes(
                virt_addr_for_phys(base as u64) as *mut u8,
                0,
                count * 0x1000,
            )
        };

        Some(Page::new(base as u64))
    }
}

impl PageAllocator for Spinlock<PageFrameAllocator> {
    fn allocate_page(&self) -> Option<Page<Size4KB>> {
        self.lock().allocate_page()
    }

    fn allocate_pages(&self, count: usize) -> Option<Page<Size4KB>> {
        self.lock().allocate_pages(count)
    }

    unsafe fn free_page(&self, page: Page<Size4KB>) {
        unsafe { self.lock().free_page(page) };
    }

    unsafe fn free_pages(&self, page: Page<Size4KB>, count: usize) {
        let mut this = self.lock();
        for p in 0..count {
            unsafe { this.free_page(Page::new(page.get_address() + p as u64 * 0x1000)) };
        }
    }
}
