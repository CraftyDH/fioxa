use core::{cmp::min, mem::size_of, ptr::slice_from_raw_parts_mut};

use bitvec::view::BitView;
use conquer_once::spin::OnceCell;

use spin::mutex::Mutex;
use uefi::table::boot::{MemoryDescriptor, MemoryType};

pub static GLOBAL_FRAME_ALLOCATOR: OnceCell<Mutex<PageFrameAllocator>> = OnceCell::uninit();

pub fn request_page() -> Option<u64> {
    let alloc = GLOBAL_FRAME_ALLOCATOR.try_get().ok()?;
    alloc.try_lock()?.request_page()
}

pub fn free_page(page: u64) -> Option<()> {
    let alloc = GLOBAL_FRAME_ALLOCATOR.try_get().ok()?;
    alloc.try_lock()?.free_page(page)
}

pub struct MemoryRegion<'bit> {
    phys_start: u64,
    page_count: u64,
    /// Bit field storing whether a frame has been allocated
    allocated: &'bit mut [u8],
}

pub struct PageFrameAllocator<'bit> {
    page_bitmap: &'bit mut [MemoryRegion<'bit>],
    free_pages: usize,
    used_pages: usize,
    /// Memory region, frame within region
    last_free_mem_region: usize,
}

impl<'mmap, 'bit> PageFrameAllocator<'bit> {
    /// Unsafe because this must only be called once (ever) since it hands out pages based on it's own state
    pub unsafe fn new(mmap: &'mmap [MemoryDescriptor]) -> Self {
        // Can inner self to get safe type checking
        Self::new_inner(mmap)
    }

    fn new_inner(mmap: &'mmap [MemoryDescriptor]) -> Self {
        // Memory types we will use
        let conventional = mmap
            .clone()
            .iter()
            .filter(|r| r.ty == MemoryType::CONVENTIONAL && r.phys_start > 0x100 * 0x1000);

        let memory_regions_cnt = conventional.clone().count();

        let total_usable_pages: u64 = conventional.clone().map(|r| r.page_count).sum();

        // How much memory will we need to store the structures?
        let bitmap_pages = (size_of::<MemoryRegion>() * memory_regions_cnt
            + (total_usable_pages / 8) as usize)
            / 4096
            + 1;

        let allocator_zone = conventional
            .clone()
            .filter(|md| md.page_count >= bitmap_pages as u64)
            .min_by(|a, b| a.page_count.cmp(&b.page_count))
            .unwrap();
        if allocator_zone.page_count < bitmap_pages as u64 {
            panic!("Max memory region doesn't have enough pages to store page allocator data");
        }

        let ptr: *mut u8 = allocator_zone.phys_start as *mut u8;

        // Ensure that we have all zeros
        unsafe { core::ptr::write_bytes(ptr, 0, bitmap_pages * 4096) }

        // Set the start of the data location for storeing the memory region headers
        let page_bitmaps =
            unsafe { &mut *slice_from_raw_parts_mut(ptr as *mut MemoryRegion, memory_regions_cnt) };

        // Counter to store to bitmaps right after each other
        let mut bitmap_idx = size_of::<MemoryRegion>() * memory_regions_cnt + 1;

        for (idx, map) in conventional.enumerate() {
            let mem_region = &mut page_bitmaps[idx];
            mem_region.phys_start = map.phys_start;
            mem_region.page_count = map.page_count;
            mem_region.allocated = unsafe {
                &mut *slice_from_raw_parts_mut(ptr.add(bitmap_idx), map.page_count as usize / 8 + 1)
            };

            bitmap_idx += map.page_count as usize / 8 + 1;
        }

        let mut alloc = Self {
            page_bitmap: page_bitmaps,
            free_pages: total_usable_pages as usize,
            used_pages: 0,
            last_free_mem_region: 0,
        };

        alloc.lock_pages(allocator_zone.phys_start, bitmap_pages as u64);
        // Ensure first 256 pages arn't handed out
        alloc.lock_pages(0, 0x100);

        println!("LOc: {}", allocator_zone.phys_start);

        println!(
            "Page allocator structures are using: {}Kb\nFree memory: {}Mb",
            bitmap_pages * 4096 / 1024,
            alloc.free_pages * 4096 / 1024 / 1024
        );

        alloc
    }

    // Returns memory address page starts at in physical memory
    pub fn request_page(&mut self) -> Option<u64> {
        for (mem_index, bitmap) in self
            .page_bitmap
            .iter_mut()
            .enumerate()
            .skip(self.last_free_mem_region)
        {
            let length = bitmap.allocated.len();

            // Skip all the chunks in the bitmap who have allready been allocated
            let mut idx = 0;
            while idx != length {
                if bitmap.allocated[idx] != 0xFF {
                    break;
                }
                idx += 1;
            }

            let bits = bitmap.allocated[idx..].view_bits_mut::<bitvec::prelude::Lsb0>();

            for (index, i) in bits.iter_mut().enumerate() {
                let page_idx = idx * 8 + index;
                // Bitmap is slightly bigger than the actual amount of pages, as it is in u8 blocks
                if page_idx + 1 > bitmap.page_count as usize {
                    break;
                };
                if !*i {
                    let idx = bitmap.phys_start + page_idx as u64 * 4096;
                    // Clear page
                    unsafe { core::ptr::write_bytes(idx as *mut u8, 0, 4096) };
                    i.commit(true);
                    self.free_pages -= 1;
                    self.used_pages += 1;

                    self.last_free_mem_region = mem_index;

                    return Some(idx);
                }
            }
        }
        None
    }

    pub fn free_page(&mut self, memory_address: u64) -> Option<()> {
        // Check it is page aligned
        assert!(memory_address % 4096 == 0);
        for (mem_index, bitmap) in self.page_bitmap.iter_mut().enumerate() {
            // Check if section contains memory address
            if bitmap.phys_start > memory_address
                || (bitmap.phys_start + bitmap.page_count * 4096) < memory_address
            {
                continue;
            }

            let bits = bitmap.allocated.view_bits_mut::<bitvec::prelude::Lsb0>();

            let page_idx = (memory_address - bitmap.phys_start) / 4096;
            let bit = bits.get_mut(page_idx as usize)?;
            if *bit {
                bit.commit(false);
                self.free_pages += 1;
                self.used_pages -= 1;

                // Improve permance with last mem region pointer
                self.last_free_mem_region = min(self.last_free_mem_region, mem_index);

                return Some(());
            } else {
                println!("WARN: tried to free unallocated page: {}", memory_address);
                return None;
            }
        }
        None
    }

    pub fn free_pages(&mut self, page_address: u64, page_cnt: u64) {
        for i in 0..page_cnt {
            self.free_page(page_address + i * 4096);
        }
    }

    pub fn lock_page(&mut self, memory_address: u64) -> Option<()> {
        // Check it is page aligned
        assert!(memory_address % 4096 == 0);
        for bitmap in self.page_bitmap.iter_mut() {
            // Check if section contains memory address
            if bitmap.phys_start > memory_address
                || (bitmap.phys_start + bitmap.page_count * 4096) < memory_address
            {
                continue;
            }

            let bits = bitmap.allocated.view_bits_mut::<bitvec::prelude::Lsb0>();

            let page_idx = (memory_address - bitmap.phys_start) / 4096;
            let bit = bits.get_mut(page_idx as usize)?;
            if !*bit {
                bit.commit(true);
                self.free_pages -= 1;
                self.used_pages += 1;
                return Some(());
            } else {
                println!("WARN: tried to lock allocated page: {}", memory_address);
                return None;
            }
        }
        None
    }

    pub fn lock_pages(&mut self, page_address: u64, page_cnt: u64) {
        for i in 0..page_cnt {
            self.lock_page(page_address + i * 4096);
        }
    }
}
