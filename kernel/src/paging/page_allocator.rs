use core::{
    cmp::min,
    ops::{Deref, DerefMut},
};

use alloc::boxed::Box;
use bit_field::{BitArray, BitField};

use bootloader::uefi::table::boot::MemoryType;
use conquer_once::spin::OnceCell;
use spin::mutex::Mutex;

use crate::{
    memory::{MemoryMapIter, MemoryMapPageIter, RESERVED_32BIT_MEM_PAGES},
    scheduling::without_context_switch,
};

use super::{
    page_table_manager::{Page, PageRange, Size4KB},
    virt_addr_for_phys, virt_addr_offset,
};

pub static BOOT_PAGE_ALLOCATOR: OnceCell<Mutex<MemoryMapPageIter>> = OnceCell::uninit();
static GLOBAL_FRAME_ALLOCATOR: OnceCell<Mutex<PageFrameAllocator>> = OnceCell::uninit();

pub fn frame_alloc_exec<T, F>(closure: F) -> T
where
    F: Fn(&mut PageFrameAllocator) -> T,
{
    without_context_switch(|| closure(&mut GLOBAL_FRAME_ALLOCATOR.get().unwrap().lock()))
}

pub unsafe fn init(mmap: MemoryMapIter) {
    let alloc = unsafe { PageFrameAllocator::new(mmap).into() };
    GLOBAL_FRAME_ALLOCATOR.init_once(|| alloc)
}

pub fn request_page() -> Option<AllocatedPage> {
    frame_alloc_exec(|mutex| mutex.request_page())
}

pub unsafe fn request_page_early() -> Option<Page<Size4KB>> {
    without_context_switch(|| match GLOBAL_FRAME_ALLOCATOR.get() {
        Some(gfa) => gfa.lock().request_page().map(|p| p.leak()),
        None => BOOT_PAGE_ALLOCATOR
            .get()
            .unwrap()
            .lock()
            .next()
            .map(|page| {
                // BOOT_PAGE_ALLOCATOR doesn't zero pages
                core::ptr::write_bytes(
                    virt_addr_for_phys(page.get_address()) as *mut u8,
                    0,
                    0x1000,
                );
                page
            }),
    })
}

pub unsafe fn free_page_early(page: Page<Size4KB>) {
    without_context_switch(|| match GLOBAL_FRAME_ALLOCATOR.get() {
        Some(gfa) => gfa.lock().free_page(page),
        None => println!("FREEING PAGE BEFORE FRAME ALLOCATOR HAS BEEN INITED (mem leak)"),
    })
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
            unsafe { frame_alloc_exec(|a| a.free_32bit_reserved_page(p)) }
        }
    }
}

#[derive(Debug, Clone)]
pub struct MemAddr {
    phys_start: u64,
    page_count: u64,
}

pub struct MemoryRegion {
    phys_start: u64,
    phys_end: u64,
    /// Bit field storing whether a frame has been allocated
    allocated: Box<[u8]>,
}

pub struct PageFrameAllocator {
    page_bitmap: Box<[MemoryRegion]>,
    /// Bitmap of memory region below 16 bits
    reserved_16bit_bitmap: u16,
    // 32 Megs of 32bit memory
    reserved_32bit: MemoryRegion,
    free_pages: usize,
    used_pages: usize,
    /// Page start addr hint (doesn't actually have to be free)
    first_free_page: u64,
}

const fn bitmap_size(elements: u64) -> u64 {
    // Fancy math to round up
    // Equivilent to
    // ((elements + 7) / 8) * 8
    (elements + 7) & !7
}

impl PageFrameAllocator {
    /// Unsafe because this must only be called once (ever) since it hands out pages based on it's own state
    pub unsafe fn new(mmap: MemoryMapIter) -> Self {
        // Can inner self to get safe type checking
        Self::new_inner(mmap)
    }

    pub fn get_free_pages(&self) -> usize {
        self.free_pages
    }

    fn new_inner(mmap: MemoryMapIter) -> Self {
        // Memory types we will use
        let mut conventional = mmap
            .map(|r| unsafe { &*virt_addr_offset(r) })
            .filter(|r| r.ty == MemoryType::CONVENTIONAL)
            .map(|r| MemAddr {
                phys_start: r.phys_start,
                page_count: r.page_count,
            });

        let u16_mem_iter = conventional
            .by_ref()
            .take_while(|r| r.phys_start <= u16::MAX.into());
        let mut u16_mem = u16::MAX;
        let mut u16_regect_mem: u64 = 0;

        // Create bitmap for 16bit mem region
        for r in u16_mem_iter {
            for mem_addr in (r.phys_start
                ..(core::cmp::min(u16::MAX.into(), r.phys_start + r.page_count * 0x1000)))
                .step_by(0x1000)
            {
                u16_mem.set_bit((mem_addr / 0x1000) as usize, false);
            }
            if r.phys_start + r.page_count * 0x1000 > u16::MAX.into() {
                assert!(u16_regect_mem == 0);
                let count = r.page_count - ((u16::MAX as u64 - r.phys_start) / 0x1000);
                u16_regect_mem = count;
                break;
            }
        }

        // If there was mem the bitmap would have been cleared at that point
        if u16_mem == u16::MAX {
            panic!("No 16 bit memory found");
        }

        // Add excess memory back into iterator
        let excess = MemAddr {
            phys_start: 1 << 16,
            page_count: u16_regect_mem,
        };

        let conventional = core::iter::once(excess).chain(conventional);

        let mut reserved_32bit_mem_region = None;

        let total_usable_pages = conventional.clone().map(|m| m.page_count as usize).sum();

        let page_bitmap: Box<[MemoryRegion]> = conventional
            .map(|mut map| {
                // the bump page alloctor doesn't hand out the first u32 page larger that reserved
                if let None = reserved_32bit_mem_region {
                    if map.phys_start <= u32::MAX.into()
                        && map.page_count >= RESERVED_32BIT_MEM_PAGES
                    {
                        reserved_32bit_mem_region = Some(MemoryRegion {
                            phys_start: map.phys_start,
                            phys_end: map.phys_start + RESERVED_32BIT_MEM_PAGES * 0x1000,
                            allocated: unsafe {
                                Box::new_zeroed_slice(bitmap_size(
                                    map.page_count - RESERVED_32BIT_MEM_PAGES,
                                ) as usize)
                                .assume_init()
                            },
                        });
                        map.phys_start += RESERVED_32BIT_MEM_PAGES * 0x1000;
                        map.page_count -= RESERVED_32BIT_MEM_PAGES;
                    }
                }
                let bit_size = bitmap_size(map.page_count) as usize;
                let allocated = unsafe {
                    let mut allocated = Box::new_uninit_slice(bit_size);
                    core::ptr::write_bytes(allocated.as_mut_ptr(), u8::MAX, bit_size);
                    allocated.assume_init()
                };
                MemoryRegion {
                    phys_start: map.phys_start,
                    phys_end: map.phys_start + map.page_count * 0x1000,
                    allocated,
                }
            })
            .collect();

        let mut alloc = Self {
            page_bitmap,
            reserved_16bit_bitmap: u16_mem,
            reserved_32bit: reserved_32bit_mem_region.expect("no reserved 32 bit section found"),
            free_pages: 0,
            used_pages: total_usable_pages,
            first_free_page: 0,
        };

        // grab all memory not already handed out and free them
        let mut bpa = BOOT_PAGE_ALLOCATOR.get().unwrap().lock();
        while let Some(page) = bpa.next() {
            unsafe { alloc.free_page(page) }
        }

        alloc
    }

    pub fn request_reserved_16bit_page(&mut self) -> Option<u16> {
        for (i, mem_location) in (0..16).map(|v| (v, v * 0x1000)) {
            if !self.reserved_16bit_bitmap.get_bit(i) {
                // Clear page
                unsafe {
                    core::ptr::write_bytes(
                        virt_addr_for_phys(mem_location as u64) as *mut u8,
                        0,
                        4096,
                    )
                };
                self.reserved_16bit_bitmap.set_bit(i, true);
                return Some(
                    mem_location
                        .try_into()
                        .expect("reserved_16bit_bitmap should only give 32bit results"),
                );
            }
        }
        None
    }

    pub fn free_reserved_16bit_page(&mut self, memory_address: u16) -> Option<()> {
        // Check it is page aligned
        assert!(memory_address % 4096 == 0);
        let idx = memory_address as usize / 0x1000;
        if self.reserved_16bit_bitmap.get_bit(idx) {
            self.reserved_16bit_bitmap.set_bit(idx, false);
            Some(())
        } else {
            panic!("WARN: tried to free unallocated page: {}", memory_address);
        }
    }

    pub fn lock_reserved_16bit_page(&mut self, memory_address: u16) -> Option<()> {
        // Check it is page aligned
        assert!(memory_address % 4096 == 0);
        let idx = memory_address as usize / 0x1000;
        if !self.reserved_16bit_bitmap.get_bit(idx) {
            self.reserved_16bit_bitmap.set_bit(idx, true);
            Some(())
        } else {
            panic!("WARN: tried to lock allocated page: {}", memory_address);
        }
    }

    fn find_page_in_region(
        current_min: u64,
        mem_region: &mut MemoryRegion,
    ) -> Option<Page<Size4KB>> {
        let page_start = core::cmp::max(current_min, mem_region.phys_start);
        let page_offset = (page_start - mem_region.phys_start) as usize / 0x1000;

        for idx in page_offset..((mem_region.phys_end - mem_region.phys_start) / 0x1000) as usize {
            if !mem_region.allocated.get_bit(idx) {
                mem_region.allocated.set_bit(idx, true);
                let page = mem_region.phys_start + idx as u64 * 0x1000;
                unsafe { core::ptr::write_bytes(virt_addr_for_phys(page) as *mut u8, 0, 0x1000) };

                return Some(Page::new(page));
            }
        }

        None
    }

    pub fn request_32bit_reserved_page(&mut self) -> Option<Allocated32Page> {
        Self::find_page_in_region(0, &mut self.reserved_32bit)
            .map(|p| unsafe { Allocated32Page::new(p) })
    }

    // Returns memory address page starts at in physical memory
    pub fn request_page(&mut self) -> Option<AllocatedPage> {
        for mem_region in self.page_bitmap.iter_mut() {
            // Check if there are potentially free pages in this region
            if self.first_free_page <= mem_region.phys_end {
                if let Some(page) = Self::find_page_in_region(self.first_free_page, mem_region) {
                    self.free_pages -= 1;
                    self.used_pages += 1;

                    // Avoid recursing the entire memory map by pointing to this addr
                    // Don't add 0x1000 because we would then need to check if the next page is valid
                    self.first_free_page = page.get_address();

                    return Some(unsafe { AllocatedPage::new(page) });
                }
            }
        }
        None
    }

    fn find_cont_page_in_region(
        current_min: u64,
        mem_region: &mut MemoryRegion,
        cnt: usize,
    ) -> Option<u64> {
        let page_start = core::cmp::max(current_min, mem_region.phys_start);
        let page_offset = (page_start - mem_region.phys_start) as usize / 0x1000;

        let mut start = page_offset;
        let mut found: usize = 0;

        for idx in page_offset..((mem_region.phys_end - mem_region.phys_start) / 0x1000) as usize {
            if !mem_region.allocated.get_bit(idx) {
                found += 1;
                if found == cnt {
                    // Set the page range as allocated
                    for i in start..=idx {
                        mem_region.allocated.set_bit(i, true);
                    }

                    let start = mem_region.phys_start + start as u64 * 0x1000;
                    unsafe {
                        core::ptr::write_bytes(
                            virt_addr_for_phys(start) as *mut u8,
                            0,
                            cnt * 0x1000,
                        )
                    };
                    return Some(start);
                }
            } else {
                found = 0;
                start = idx + 1;
            }
        }
        None
    }

    pub fn request_cont_pages(&mut self, cnt: usize) -> Option<AllocatedPageRangeIter> {
        for mem_region in self.page_bitmap.iter_mut() {
            if self.first_free_page <= mem_region.phys_end {
                if let Some(page) =
                    Self::find_cont_page_in_region(self.first_free_page, mem_region, cnt)
                {
                    self.free_pages -= cnt;
                    self.used_pages += cnt;
                    // We don't set first_free_page, because we might've skipped some free pages in the search for cont pages
                    return Some(AllocatedPageRangeIter(PageRange::new(page, cnt)));
                }
            }
        }
        None
    }

    fn free_page_in_region(mem_region: &mut MemoryRegion, memory_address: u64) -> Option<()> {
        let page_idx = (memory_address - mem_region.phys_start) / 4096;
        if mem_region.allocated.get_bit(page_idx as usize) {
            mem_region.allocated.set_bit(page_idx as usize, false);

            Some(())
        } else {
            panic!("WARN: tried to free unallocated page: {}", memory_address);
        }
    }

    fn lock_page_in_region(mem_region: &mut MemoryRegion, memory_address: u64) -> Option<()> {
        let page_idx = (memory_address - mem_region.phys_start) / 4096;
        if !mem_region.allocated.get_bit(page_idx as usize) {
            mem_region.allocated.set_bit(page_idx as usize, true);

            Some(())
        } else {
            panic!("WARN: tried to lock allocated page: {}", memory_address);
        }
    }

    pub unsafe fn free_32bit_reserved_page(&mut self, page: Page<Size4KB>) {
        assert!(page.get_address() <= u32::MAX.into());
        Self::free_page_in_region(&mut self.reserved_32bit, page.get_address()).unwrap();
    }

    pub unsafe fn free_page(&mut self, page: Page<Size4KB>) {
        let memory_address = page.get_address();
        for mem_region in self.page_bitmap.iter_mut() {
            // Check if section contains memory address
            if (mem_region.phys_start..=mem_region.phys_end).contains(&memory_address) {
                if let Some(()) = Self::free_page_in_region(mem_region, memory_address) {
                    self.free_pages += 1;
                    self.used_pages -= 1;

                    // Improve allocator performance by setting last free mem region as this if less
                    self.first_free_page = min(self.first_free_page, memory_address);
                    return;
                } else {
                    panic!("WARN: tried to free unallocated page: {}", memory_address);
                };
            }
        }
        panic!("went through whole mem and couldn't find page");
    }

    // pub fn free_pages(&mut self, page_address: u64, page_cnt: u64) {
    //     for i in 0..page_cnt {
    //         self.free_page(page_address + i * 4096)
    //     }
    // }

    pub fn lock_page(&mut self, memory_address: u64) -> Option<()> {
        // Check it is page aligned
        assert!(memory_address % 4096 == 0);
        for mem_region in self.page_bitmap.iter_mut() {
            // Check if section contains memory address
            if (mem_region.phys_start..=mem_region.phys_end).contains(&memory_address) {
                return if let Some(()) = Self::lock_page_in_region(mem_region, memory_address) {
                    self.free_pages -= 1;
                    self.used_pages += 1;
                    Some(())
                } else {
                    panic!("WARN: tried to lock unallocated page: {}", memory_address);
                };
            }
        }
        None
    }

    pub fn lock_pages(&mut self, page_address: u64, page_cnt: u64) -> Option<()> {
        for i in 0..page_cnt {
            self.lock_page(page_address + i * 4096)?
        }
        Some(())
    }
}
