use core::{
    cmp::min,
    mem::{size_of, MaybeUninit},
    ptr::slice_from_raw_parts_mut,
};

use bit_field::{BitArray, BitField};

use spin::mutex::Mutex;
use uefi::table::boot::MemoryType;

use crate::{memory::MemoryMapIter, scheduling::without_context_switch};

use super::{virt_addr_for_phys, MemoryLoc};

static GLOBAL_FRAME_ALLOCATOR: Mutex<MaybeUninit<PageFrameAllocator>> =
    Mutex::new(MaybeUninit::uninit());

const RESERVED_32BIT_MEM_PAGES: u64 = 32; // 16Kb

pub fn frame_alloc_exec<T, F>(closure: F) -> Option<T>
where
    F: Fn(&mut PageFrameAllocator) -> Option<T>,
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

pub fn request_page() -> Option<u64> {
    frame_alloc_exec(|mutex| mutex.request_page())
}

pub fn free_page(page: u64) -> Option<()> {
    frame_alloc_exec(|mutex| mutex.free_page(page))
}

#[derive(Debug, Clone)]
pub struct MemAddr {
    phys_start: u64,
    page_count: u64,
}

pub struct MemoryRegion<'bit> {
    phys_start: u64,
    page_count: u64,
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
    /// Memory region, frame within region
    last_free_mem_region: usize,
}

const fn bitmap_size(elements: u64) -> u64 {
    // Fancy math to round up
    // Equivilent to
    // ((elements + 7) / 8) * 8
    (elements + 7) & !7
}

impl<'mmap, 'bit> PageFrameAllocator<'bit> {
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
        reserved_32bit_mem_region.page_count = RESERVED_32BIT_MEM_PAGES;
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
            mem_region.page_count = map.page_count;
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
            last_free_mem_region: 0,
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
            println!("WARN: tried to free unallocated page: {}", memory_address);
            None
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
            println!("WARN: tried to lock allocated page: {}", memory_address);
            None
        }
    }

    fn find_page_in_region(mem_region: &mut MemoryRegion) -> Option<u64> {
        for (bits, base_page) in mem_region.allocated.iter_mut().zip((0..).step_by(8)) {
            // Check if all bits in section are allocated
            if *bits == 0xFF {
                continue;
            }
            for i in 0..8 {
                if !bits.get_bit(i) {
                    // Ensure we arn't passed page count size bitmap has a bit of padding
                    if base_page + i + 1 > mem_region.page_count as usize {
                        return None;
                    }
                    bits.set_bit(i, true);
                    let loc = mem_region.phys_start + ((base_page + i) * 0x1000) as u64;
                    unsafe {
                        core::ptr::write_bytes(virt_addr_for_phys(loc) as *mut u8, 0, 0x1000)
                    };

                    return Some(loc);
                }
            }
        }
        None
    }

    pub fn request_32bit_reserved_page(&mut self) -> Option<u32> {
        Some(Self::find_page_in_region(self.reserved_32bit)? as u32)
    }

    // Returns memory address page starts at in physical memory
    pub fn request_page(&mut self) -> Option<u64> {
        for (mem_index, mem_region) in self
            .page_bitmap
            .iter_mut()
            .enumerate()
            .skip(self.last_free_mem_region)
        {
            if let Some(page) = Self::find_page_in_region(mem_region) {
                self.free_pages -= 1;
                self.used_pages += 1;

                // Avoid recursing the entire memory map by pointing to this mem region
                self.last_free_mem_region = mem_index;

                return Some(page);
            }
        }
        None
    }

    fn find_cont_page_in_region(mem_region: &mut MemoryRegion, cnt: usize) -> Option<u64> {
        let mut n = 0;
        let mut start = 0;
        let mut last = 0;
        for (bits, base_page) in mem_region.allocated.iter_mut().zip((0..).step_by(8)) {
            // Check if all bits in section are allocated
            if *bits == 0xFF {
                continue;
            }
            for i in 0..8 {
                if !bits.get_bit(i) && last == base_page + i {
                    // Ensure we arn't passed page count size bitmap has a bit of padding
                    if base_page + i + 1 > mem_region.page_count as usize {
                        return None;
                    }
                    n += 1;
                    last += 1;
                    if n == cnt {
                        for (bits, base_page) in
                            mem_region.allocated.iter_mut().zip((0..).step_by(8))
                        {
                            for i in 0..8 {
                                if base_page + i >= start && base_page + i <= last {
                                    bits.set_bit(i, true);
                                }
                            }
                        }
                        let start = mem_region.phys_start + start as u64 * 0x1000;
                        let last = mem_region.phys_start + last as u64 * 0x1000;
                        unsafe {
                            core::ptr::write_bytes(
                                virt_addr_for_phys(start) as *mut u8,
                                0,
                                (last - start) as usize,
                            )
                        };
                        return Some(start as u64);
                    }
                } else {
                    n = 0;
                    start = base_page + i + 1;
                    last = start;
                }
            }
        }
        None
    }

    pub fn request_cont_pages(&mut self, cnt: usize) -> Option<u64> {
        for (mem_index, mem_region) in self
            .page_bitmap
            .iter_mut()
            .enumerate()
            .skip(self.last_free_mem_region)
        {
            if let Some(page) = Self::find_cont_page_in_region(mem_region, cnt) {
                self.free_pages -= cnt;
                self.used_pages += cnt;

                // Avoid recursing the entire memory map by pointing to this mem region
                self.last_free_mem_region = mem_index;

                return Some(page);
            }
        }
        None
    }

    fn free_page_in_region(mem_region: &mut MemoryRegion, memory_address: u64) -> Option<()> {
        let page_idx = (memory_address - mem_region.phys_start) / 4096;
        if mem_region.allocated.get_bit(page_idx as usize) {
            mem_region.allocated.set_bit(page_idx as usize, false);

            return Some(());
        } else {
            println!("WARN: tried to free unallocated page: {}", memory_address);
            return None;
        }
    }

    fn lock_page_in_region(mem_region: &mut MemoryRegion, memory_address: u64) -> Option<()> {
        let page_idx = (memory_address - mem_region.phys_start) / 4096;
        if !mem_region.allocated.get_bit(page_idx as usize) {
            mem_region.allocated.set_bit(page_idx as usize, true);

            return Some(());
        } else {
            println!("WARN: tried to lock allocated page: {}", memory_address);
            return None;
        }
    }

    pub fn free_32bit_reserved_page(&mut self, memory_address: u32) -> Option<()> {
        // Check it is page aligned
        assert!(memory_address % 4096 == 0);
        Self::free_page_in_region(self.reserved_32bit, memory_address as u64)
    }

    pub fn free_page(&mut self, memory_address: u64) -> Option<()> {
        // Check it is page aligned
        assert!(memory_address % 4096 == 0);
        for (mem_index, mem_region) in self.page_bitmap.iter_mut().enumerate() {
            // Check if section contains memory address
            if mem_region.phys_start > memory_address
                || (mem_region.phys_start + mem_region.page_count * 4096) < memory_address
            {
                continue;
            }

            return if let Some(()) = Self::free_page_in_region(mem_region, memory_address) {
                self.free_pages += 1;
                self.used_pages -= 1;

                // Improve allocator performance by setting last free mem region as this if less
                self.last_free_mem_region = min(self.last_free_mem_region, mem_index);
                Some(())
            } else {
                println!("WARN: tried to free unallocated page: {}", memory_address);
                None
            };
        }
        None
    }

    pub fn free_pages(&mut self, page_address: u64, page_cnt: u64) -> Option<()> {
        for i in 0..page_cnt {
            self.free_page(page_address + i * 4096)?;
        }
        Some(())
    }

    pub fn lock_page(&mut self, memory_address: u64) -> Option<()> {
        // Check it is page aligned
        assert!(memory_address % 4096 == 0);
        for mem_region in self.page_bitmap.iter_mut() {
            // Check if section contains memory address
            if mem_region.phys_start > memory_address
                || (mem_region.phys_start + mem_region.page_count * 4096) < memory_address
            {
                continue;
            }

            return if let Some(()) = Self::lock_page_in_region(mem_region, memory_address) {
                self.free_pages -= 1;
                self.used_pages += 1;
                Some(())
            } else {
                None
            };
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
