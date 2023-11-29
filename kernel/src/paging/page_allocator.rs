use core::ops::{Deref, DerefMut};

use alloc::vec::Vec;
use bootloader::{MemoryClass, MemoryMapEntrySlice};
use conquer_once::spin::OnceCell;
use spin::mutex::Mutex;

use crate::{memory::RESERVED_32BIT_MEM_PAGES, scheduling::without_context_switch};

use super::{
    get_uefi_active_mapper,
    page_table_manager::{Page, PageRange, Size4KB},
    virt_addr_for_phys, virt_addr_offset_mut, MemoryLoc, KERNEL_HEAP_MAP,
};

static GLOBAL_FRAME_ALLOCATOR: OnceCell<Mutex<PageFrameAllocator>> = OnceCell::uninit();

pub fn frame_alloc_exec<T, F>(closure: F) -> T
where
    F: Fn(&mut PageFrameAllocator) -> T,
{
    without_context_switch(|| closure(&mut GLOBAL_FRAME_ALLOCATOR.get().unwrap().lock()))
}

pub unsafe fn init(mmap: &MemoryMapEntrySlice) {
    let alloc = unsafe { PageFrameAllocator::new(mmap).into() };
    GLOBAL_FRAME_ALLOCATOR.init_once(|| alloc);

    // ensure that allocations that happen during init carry over
    let mut uefi = get_uefi_active_mapper();
    uefi.set_next_table(MemoryLoc::KernelHeap as u64, &mut KERNEL_HEAP_MAP.lock());

    let free = mmap
        .iter()
        .map(|e| &*e)
        .map(|e| SectionPageMapping {
            phys_start: e.phys_start as usize,
            page_count: e.page_count as usize,
            map_type: e.class,
        })
        .collect();

    GLOBAL_FRAME_ALLOCATOR.get_unchecked().lock().mappings = free;
}

pub fn request_page() -> Option<AllocatedPage> {
    frame_alloc_exec(|mutex| mutex.request_page())
}

pub unsafe fn free_page_early(page: Page<Size4KB>) {
    frame_alloc_exec(|mutex| mutex.free_page(page))
}

pub struct AllocatedPage(Option<Page<Size4KB>>);

impl AllocatedPage {
    pub unsafe fn new(page: Page<Size4KB>) -> Self {
        Self(Some(page))
    }

    pub unsafe fn leak(mut self) -> Page<Size4KB> {
        self.0.take().expect("should always be some")
    }
}

impl Deref for AllocatedPage {
    type Target = Page<Size4KB>;

    fn deref(&self) -> &Self::Target {
        self.0.as_ref().expect("should always be some")
    }
}

impl DerefMut for AllocatedPage {
    fn deref_mut(&mut self) -> &mut Self::Target {
        self.0.as_mut().expect("should always be some")
    }
}

impl Drop for AllocatedPage {
    fn drop(&mut self) {
        if let Some(p) = self.0 {
            unsafe { frame_alloc_exec(|a| a.free_page(p)) }
        }
    }
}

#[derive(Debug)]
pub struct AllocatedPageRangeIter(PageRange<Size4KB>);

impl Iterator for AllocatedPageRangeIter {
    type Item = AllocatedPage;

    fn next(&mut self) -> Option<Self::Item> {
        self.0
            .next()
            .map(|page| unsafe { AllocatedPage::new(page) })
    }

    fn size_hint(&self) -> (usize, Option<usize>) {
        self.0.size_hint()
    }
}

impl ExactSizeIterator for AllocatedPageRangeIter {
    fn len(&self) -> usize {
        self.0.len()
    }
}

impl Drop for AllocatedPageRangeIter {
    fn drop(&mut self) {
        while let Some(_) = self.0.next() {}
    }
}

pub struct Allocated32Page(Option<Page<Size4KB>>);

impl Allocated32Page {
    pub unsafe fn new(page: Page<Size4KB>) -> Self {
        Self(Some(page))
    }

    pub unsafe fn leak(mut self) -> Page<Size4KB> {
        self.0.take().expect("should always be some")
    }

    pub fn get_address(&self) -> u32 {
        self.0
            .expect("should always be some")
            .get_address()
            .try_into()
            .expect("should always be able to fit")
    }
}

impl Deref for Allocated32Page {
    type Target = Page<Size4KB>;

    fn deref(&self) -> &Self::Target {
        self.0.as_ref().expect("should always be some")
    }
}

impl DerefMut for Allocated32Page {
    fn deref_mut(&mut self) -> &mut Self::Target {
        self.0.as_mut().expect("should always be some")
    }
}

impl Drop for Allocated32Page {
    fn drop(&mut self) {
        if let Some(p) = self.0 {
            unsafe { frame_alloc_exec(|a| a.free_32bit_reserved_page(p.get_address() as usize)) }
        }
    }
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
    free_lists: [Option<*mut PageMetadata>; MAX_ORDER],
    reserved_32bit: Option<*mut PageMetadata32>,

    // we reserve 0x8000 specifically for the purpose of booting AP's
    captured_0x8000: bool,
    total_free: usize,
    mappings: Vec<SectionPageMapping>,
}

pub struct SectionPageMapping {
    phys_start: usize,
    page_count: usize,
    map_type: MemoryClass,
}

unsafe impl Send for PageFrameAllocator {}

#[inline(always)]
pub fn pages_in_order(order: usize) -> usize {
    1 << order
}

impl PageFrameAllocator {
    pub unsafe fn new(mmap: &MemoryMapEntrySlice) -> Self {
        let mut this = Self {
            free_lists: Default::default(),
            captured_0x8000: false,
            reserved_32bit: None,
            total_free: 0,
            // This is fine as vec will not allocate until pushed
            mappings: Vec::new(),
        };

        let mut free = mmap
            .iter()
            .map(|e| &*e)
            .filter(|e| e.class == MemoryClass::Free);

        // Capture the reserved pages
        let mut free_found = 0;
        loop {
            let entry = free.by_ref().next().unwrap();

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
                .for_each(|p| this.free_32bit_reserved_page(p));

            if free_found == RESERVED_32BIT_MEM_PAGES {
                let amount_left = entry.page_count as usize - taken_amount;
                this.total_free += amount_left;
                this.insert_free_of_range(
                    entry.phys_start as usize + taken_amount * 0x1000,
                    amount_left,
                );
                break;
            } else if free_found > RESERVED_32BIT_MEM_PAGES {
                unreachable!("logic error")
            }
        }

        for entry in free {
            let start_addr = entry.phys_start as usize;
            let pages_left = entry.page_count as usize;

            this.total_free += pages_left;
            this.insert_free_of_range(start_addr, pages_left);
        }
        this
    }

    pub unsafe fn reclaim_memory(&mut self) -> usize {
        let mut map = core::mem::take(&mut self.mappings);

        let mut reclaim = 0;
        let mut last = None;
        for el in map.iter_mut() {
            match el.map_type {
                MemoryClass::Free => last = Some(el),
                MemoryClass::KernelReclaim => {
                    self.total_free += el.page_count;
                    reclaim += el.page_count;
                    self.insert_free_of_range(el.phys_start, el.page_count);

                    if last
                        .as_ref()
                        .is_some_and(|l| l.phys_start + l.page_count * 0x1000 == el.phys_start)
                    {
                        // The last node is good for collapsing
                        last.as_mut().unwrap().page_count += el.page_count;
                    } else {
                        el.map_type = MemoryClass::Free;
                        last = Some(el);
                    }
                }
                _ => last = None,
            }
        }

        map.retain(|e| e.map_type != MemoryClass::KernelReclaim);
        self.mappings = map;
        reclaim
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

            self.insert_free_of_order(start_addr, order);

            let page_count = pages_in_order(order);
            pages_left -= page_count;
            start_addr += page_count * 0x1000;
        }
    }

    unsafe fn insert_free_of_order(&mut self, base: usize, order: usize) {
        let left = &mut *virt_addr_offset_mut(base as *mut PageMetadata);
        left.order = order;
        left.next_node = None;
        left.prev_node = None;

        // if order is 0, we only have 1 page
        if order > 0 {
            let last_page_addr_offset = (pages_in_order(order) - 1) * 0x1000;
            let right =
                &mut *virt_addr_offset_mut((base + last_page_addr_offset) as *mut PageMetadata);
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

        Some(AllocatedPageOrder { order, base: base })
    }

    pub fn request_page(&mut self) -> Option<AllocatedPage> {
        let base = self.request_page_of_order(0)?.base as u64;

        unsafe { core::ptr::write_bytes(virt_addr_for_phys(base) as *mut u8, 0, 0x1000) };

        Some(AllocatedPage(Some(Page::new(base))))
    }

    pub unsafe fn free_32bit_reserved_page(&mut self, page: usize) {
        let meta = &mut *virt_addr_offset_mut(page as *mut PageMetadata32);

        if let Some(p) = self.reserved_32bit {
            meta.next_node = Some(p);
            self.reserved_32bit = Some(page as *mut PageMetadata32);
        } else {
            self.reserved_32bit = Some(page as *mut PageMetadata32);
        }
    }

    pub fn request_32bit_reserved_page(&mut self) -> Option<Allocated32Page> {
        let block = self.reserved_32bit?;
        let b = unsafe { &mut *virt_addr_offset_mut(block) };
        let base = block as *const _ as u64;
        self.reserved_32bit = b.next_node;
        unsafe { core::ptr::write_bytes(virt_addr_for_phys(base) as *mut u8, 0, 0x1000) };
        Some(Allocated32Page(Some(Page::new(base))))
    }

    pub fn free_page_of_order(&mut self, pages: AllocatedPageOrder) {
        unsafe { self.insert_free_of_order(pages.base, pages.order) }
    }

    pub unsafe fn free_page(&mut self, page: Page<Size4KB>) {
        unsafe { self.insert_free_of_order(page.get_address() as usize, 0) }
    }

    pub fn request_cont_pages(&mut self, count: usize) -> Option<AllocatedPageRangeIter> {
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

        Some(AllocatedPageRangeIter(PageRange::new(base as u64, count)))
    }
}
