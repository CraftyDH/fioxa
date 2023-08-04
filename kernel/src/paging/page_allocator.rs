use core::{
    cmp::min,
    mem::{size_of, MaybeUninit},
    ops::{Deref, DerefMut},
    ptr::slice_from_raw_parts_mut,
};

use bit_field::{BitArray, BitField};

use bootloader::uefi::table::boot::MemoryType;
use spin::mutex::Mutex;

use crate::{memory::MemoryMapIter, scheduling::without_context_switch};

use super::{
    page_table_manager::{Page, Size4KB},
    virt_addr_for_phys, MemoryLoc,
};

static GLOBAL_FRAME_ALLOCATOR: Mutex<MaybeUninit<PageFrameAllocator>> =
    Mutex::new(MaybeUninit::uninit());

const RESERVED_32BIT_MEM_PAGES: u64 = 32; // 16Kb

pub fn frame_alloc_exec<T, F>(closure: F) -> T
where
    F: Fn(&mut PageFrameAllocator) -> T,
{
    without_context_switch(|| unsafe {
        closure(&mut *GLOBAL_FRAME_ALLOCATOR.lock().assume_init_mut())
    })
}

pub unsafe fn init(mmap: MemoryMapIter) {
    GLOBAL_FRAME_ALLOCATOR
        .lock()
        .write(unsafe { PageFrameAllocator::new(mmap.clone()) });
}

pub fn request_page() -> Option<AllocatedPage> {
    frame_alloc_exec(|mutex| mutex.request_page())
}

pub struct AllocatedPage(Option<Page<Size4KB>>);

impl AllocatedPage {
    pub unsafe fn new(page: Page<Size4KB>) -> Self {
        Self(Some(page))
    }

    pub unsafe fn leak(mut self) -> Page<Size4KB> {
        self.0.take().unwrap()
    }
}

impl Deref for AllocatedPage {
    type Target = Page<Size4KB>;

    fn deref(&self) -> &Self::Target {
        self.0.as_ref().unwrap()
    }
}

impl DerefMut for AllocatedPage {
    fn deref_mut(&mut self) -> &mut Self::Target {
        self.0.as_mut().unwrap()
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
pub struct AllocatedPageRangeIter {
    base: u64,
    count: usize,
}

impl Iterator for AllocatedPageRangeIter {
    type Item = AllocatedPage;

    fn next(&mut self) -> Option<Self::Item> {
        if self.count > 0 {
            let res = unsafe { AllocatedPage::new(Page::new(self.base)) };
            self.count -= 1;
            self.base += 0x1000;
            Some(res)
        } else {
            None
        }
    }

    fn size_hint(&self) -> (usize, Option<usize>) {
        (self.count, Some(self.count))
    }
}

impl ExactSizeIterator for AllocatedPageRangeIter {
    fn len(&self) -> usize {
        self.count
    }
}

pub struct Allocated32Page(Option<Page<Size4KB>>);

impl Allocated32Page {
    pub unsafe fn new(page: Page<Size4KB>) -> Self {
        Self(Some(page))
    }

    pub unsafe fn leak(mut self) -> Page<Size4KB> {
        self.0.take().unwrap()
    }

    pub fn get_address(&self) -> u32 {
        self.0.unwrap().get_address().try_into().unwrap()
    }
}

impl Deref for Allocated32Page {
    type Target = Page<Size4KB>;

    fn deref(&self) -> &Self::Target {
        self.0.as_ref().unwrap()
    }
}

impl DerefMut for Allocated32Page {
    fn deref_mut(&mut self) -> &mut Self::Target {
        self.0.as_mut().unwrap()
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

pub struct MemoryRegion<'bit> {
    phys_start: u64,
    phys_end: u64,
    /// Bit field storing whether a frame has been allocated
    allocated: &'bit mut [u8],
}

pub struct PageFrameAllocator<'bit> {
    page_bitmap: &'bit mut [MemoryRegion<'bit>],
    /// Bitmap of memory region below 16 bits
    reserved_16bit_bitmap: u16,
    // 32 Megs of 32bit memory
    reserved_32bit: &'bit mut MemoryRegion<'bit>,
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

impl PageFrameAllocator<'_> {
    /// Unsafe because this must only be called once (ever) since it hands out pages based on it's own state
    pub unsafe fn new(mmap: MemoryMapIter) -> Self {
        // Can inner self to get safe type checking
        Self::new_inner(mmap)
    }

    pub unsafe fn push_up_to_offset_mapping(&mut self) {
        for bit in self.page_bitmap.iter_mut() {
            bit.allocated = &mut *slice_from_raw_parts_mut(
                bit.allocated
                    .as_mut_ptr()
                    .byte_add(MemoryLoc::PhysMapOffset as usize),
                bit.allocated.len(),
            );
        }

        self.page_bitmap = &mut *slice_from_raw_parts_mut(
            self.page_bitmap
                .as_mut_ptr()
                .byte_add(MemoryLoc::PhysMapOffset as usize),
            self.page_bitmap.len(),
        );
        self.reserved_32bit.allocated = &mut *slice_from_raw_parts_mut(
            self.reserved_32bit
                .allocated
                .as_mut_ptr()
                .byte_add(MemoryLoc::PhysMapOffset as usize),
            self.reserved_32bit.allocated.len(),
        );

        self.reserved_32bit = &mut *(self.reserved_32bit as *mut MemoryRegion)
            .byte_add(MemoryLoc::PhysMapOffset as usize);
    }

    pub fn get_free_pages(&self) -> usize {
        self.free_pages
    }

    fn new_inner(mmap: MemoryMapIter) -> Self {
        // Memory types we will use
        let mut conventional = mmap
            .filter(|r| r.ty == MemoryType::CONVENTIONAL)
            .map(|r| MemAddr {
                phys_start: r.phys_start,
                page_count: r.page_count,
            });

        let u16_mem_iter = conventional.by_ref().take_while(|r| r.phys_start < 1 << 16);
        let mut u16_mem = u16::MAX;
        let mut u16_regect_mem: u64 = 0;

        // Create bitmap for 16bit mem region
        for r in u16_mem_iter {
            for mem_addr in (r.phys_start
                ..(core::cmp::min(1 << 16, r.phys_start + r.page_count * 0x1000)))
                .step_by(0x1000)
            {
                u16_mem.set_bit((mem_addr / 0x1000) as usize, false);
            }
            if r.phys_start + r.page_count * 0x1000 > 1 << 16 {
                assert!(u16_regect_mem == 0);
                let count = r.page_count - (((1 << 16) - r.phys_start) / 0x1000);
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
        let mut conventional = core::iter::once(excess).chain(conventional);

        // Descriptor for reserved 32bit split
        let memory_regions_cnt = conventional.clone().count() + 1;

        let size_of_bitmaps: u64 = conventional
            .clone()
            .map(|r| bitmap_size(r.page_count))
            .sum();

        // How much memory will we need to store the structures?
        let bitmap_pages = (size_of::<MemoryRegion>() * memory_regions_cnt
            + size_of_bitmaps as usize)
            / 4096
            // Add and extra page to be safe
            + 1;

        let allocator_zone = conventional
            .clone()
            .filter(|md| md.page_count >= bitmap_pages as u64)
            .min_by(|a, b| a.page_count.cmp(&b.page_count))
            .unwrap();
        if allocator_zone.page_count < bitmap_pages as u64 {
            panic!("Max continuous memory region doesn't have enough pages to store page allocator data");
        }

        let ptr: *mut u8 = allocator_zone.phys_start as *mut u8;

        // Ensure that we have all zeros
        unsafe { core::ptr::write_bytes(ptr, 0, bitmap_pages * 4096) }

        let reserved_32bit_mem_region = unsafe { &mut *(ptr as *mut MemoryRegion) };

        // Set the start of the data location for storeing the memory region headers after reserved region
        let page_bitmaps = unsafe {
            &mut *slice_from_raw_parts_mut(
                ptr.add(size_of::<MemoryRegion>()) as *mut MemoryRegion,
                memory_regions_cnt - 1,
            )
        };

        // Counter to store to bitmaps right after each other
        let mut bitmap_idx = size_of::<MemoryRegion>() * memory_regions_cnt + 1;

        let reserved_32bit = conventional
            .by_ref()
            .find(|r| r.page_count >= RESERVED_32BIT_MEM_PAGES)
            .unwrap();

        reserved_32bit_mem_region.phys_start = reserved_32bit.phys_start;
        reserved_32bit_mem_region.phys_end =
            reserved_32bit.phys_start + RESERVED_32BIT_MEM_PAGES * 0x1000;
        let bitmap_size_pages = bitmap_size(RESERVED_32BIT_MEM_PAGES) as usize;
        reserved_32bit_mem_region.allocated =
            unsafe { &mut *slice_from_raw_parts_mut(ptr.add(bitmap_idx), bitmap_size_pages) };
        bitmap_idx += bitmap_size_pages;

        // Add excess memory back into iterator
        let excess = MemAddr {
            phys_start: reserved_32bit.phys_start + RESERVED_32BIT_MEM_PAGES * 0x1000,
            page_count: reserved_32bit.page_count - RESERVED_32BIT_MEM_PAGES,
        };
        let conventional = core::iter::once(excess).chain(conventional);

        // Count user usable pages
        let mut total_usable_pages: usize = 0;

        for (idx, map) in conventional.enumerate() {
            println!("{:?}", map);
            let mem_region = &mut page_bitmaps[idx];
            mem_region.phys_start = map.phys_start;
            mem_region.phys_end = map.phys_start + map.page_count * 0x1000;
            total_usable_pages += map.page_count as usize;

            let bitmap_size_pages: usize = bitmap_size(map.page_count) as usize;

            mem_region.allocated =
                unsafe { &mut *slice_from_raw_parts_mut(ptr.add(bitmap_idx), bitmap_size_pages) };

            bitmap_idx += bitmap_size_pages;
        }

        let mut alloc = Self {
            page_bitmap: page_bitmaps,
            reserved_16bit_bitmap: u16_mem,
            reserved_32bit: reserved_32bit_mem_region,
            free_pages: total_usable_pages,
            used_pages: 0,
            first_free_page: 0,
        };

        alloc.lock_pages(allocator_zone.phys_start, bitmap_pages as u64);

        println!(
            "Page allocator structures are using: {}Kb\nFree memory: {}Mb",
            bitmap_pages * 4096 / 1024,
            alloc.free_pages * 4096 / 1024 / 1024
        );

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
                return Some(mem_location.try_into().unwrap());
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
        Self::find_page_in_region(0, self.reserved_32bit)
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
                    return Some(AllocatedPageRangeIter {
                        base: page,
                        count: cnt,
                    });
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
        Self::free_page_in_region(self.reserved_32bit, page.get_address()).unwrap();
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
