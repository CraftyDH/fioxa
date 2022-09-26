use core::{mem::MaybeUninit, ptr::slice_from_raw_parts_mut};

use bitvec::view::BitView;
use conquer_once::spin::OnceCell;
use spin::mutex::Mutex;
use uefi::table::boot::{MemoryDescriptor, MemoryType};

use crate::memory;

pub static GLOBAL_FRAME_ALLOCATOR: OnceCell<Mutex<PageFrameAllocator>> = OnceCell::uninit();

pub fn request_page() -> Option<usize> {
    if GLOBAL_FRAME_ALLOCATOR.is_poisoned() {
        println!("GLOBAL ALLOC POISOED");
        return None;
    }
    let alloc = GLOBAL_FRAME_ALLOCATOR.get()?;
    // print!("P");
    let x = alloc.lock().request_page();
    // print!("G");
    x
}

pub fn request_pages(number: usize) -> Option<usize> {
    let alloc = GLOBAL_FRAME_ALLOCATOR.get()?;
    alloc.lock().request_pages(number)
}

pub fn free_page(page: usize) -> Option<()> {
    let alloc = GLOBAL_FRAME_ALLOCATOR.get()?;
    alloc.lock().free_page(page);
    Some(())
}

pub fn free_pages(pages: usize, number: usize) -> Option<()> {
    let alloc = GLOBAL_FRAME_ALLOCATOR.get()?;
    alloc.lock().free_pages(pages, number);
    Some(())
}

pub struct PageFrameAllocator<'mmap, 'bit> {
    mmap: &'mmap [MemoryDescriptor],
    page_bitmap: &'bit mut [u64],
    free_pages: usize,
    used_pages: usize,
    reserved_pages: usize,
    last_page_used: usize,
}

fn usable_frames(mmap: &'_ [MemoryDescriptor]) -> impl Iterator<Item = u64> + '_ {
    let usable_regions = mmap
        .clone()
        .iter()
        .filter(|r| r.ty == MemoryType::CONVENTIONAL && r.phys_start <= 100_000_000);

    let addr_ranges = usable_regions
        .clone()
        .map(|r| r.phys_start..(r.phys_start + r.page_count * 4096));

    let pages = addr_ranges.flat_map(|r| r.step_by(4096));
    pages
}

impl<'mmap, 'bit> PageFrameAllocator<'mmap, 'bit> {
    pub fn new(mmap: &'mmap [MemoryDescriptor]) -> Self {
        let memory_size = memory::get_memory_size_pages(mmap);

        let mut usable_frame = usable_frames(mmap);

        // We ignore above this range other wise our bitmap explodes in size
        let max = usable_frames(mmap).max().unwrap();

        let bitmap_pages = max as usize / 4096 / 8 + 1;

        let mut bitmap_start_frame = 0;
        let mut next_frame = 0;
        let mut pages = 0;

        while let Some(pg) = usable_frame.next() {
            let frame = pg / 4096;
            if frame == 0 {
                continue;
            }
            if frame - 1 == next_frame {
                pages += 1;
                if pages == bitmap_pages {
                    break;
                }
            } else {
                bitmap_start_frame = frame;
                pages = 0;
            }
            next_frame = frame;
        }

        if pages != bitmap_pages {
            panic!("Not enough pages for bitmap")
        }

        println!("Enough pages");

        let buf = unsafe {
            let ptr: *mut u64 = (bitmap_start_frame * 4096) as *mut u64;
            // core::ptr::from_exposed_addr(addr)
            core::ptr::write_bytes(ptr, 0xFF, bitmap_pages * 4096 / 8);
            &mut *slice_from_raw_parts_mut(ptr, bitmap_pages * 4096 / 8)
        };

        // buf.fill(0xFF);
        {
            let x = buf.view_bits_mut::<bitvec::prelude::Lsb0>();

            let y = x.get(max as usize);
            println!("{:?}", y)
        }
        let mut alloc = Self {
            mmap,
            page_bitmap: buf,
            free_pages: 0,
            used_pages: 0,
            reserved_pages: memory_size as usize,
            last_page_used: 0,
        };

        // Only unlock pages we like
        // Much better than lock not conventional
        // as this ensures gaps in the UEFI memory map don't break our allocator
        for mb in mmap {
            if mb.ty == MemoryType::CONVENTIONAL {
                if mb.phys_start >= 100_000_000 {
                    continue;
                }
                alloc.unreserve_pages(mb.phys_start as usize / 4096, mb.page_count as usize);
            }
        }

        println!(
            "Bitmap start: {}, pages: {}",
            bitmap_start_frame, bitmap_pages
        );
        alloc.lock_pages(bitmap_start_frame as usize, bitmap_pages);
        // Ensure page 0 isn't handed out
        alloc.reserve_page(0);

        println!("Free: {}", alloc.free_pages);
        println!("Locked: {}", alloc.used_pages);
        println!("Reserved: {}", alloc.reserved_pages);

        alloc
    }

    pub fn request_page(&mut self) -> Option<usize> {
        let page = self.find_page()?;
        self.lock_page(page);
        let page = page * 4096;
        // Zero page
        // let p = unsafe { &mut *slice_from_raw_parts_mut(page as *mut u64, 4096 / 64) };
        // p.fill(0);
        unsafe { core::ptr::write_bytes(page as *mut u64, 0, 4096 / 8) };
        Some(page)
    }

    fn find_page(&mut self) -> Option<usize> {
        let bits = self.page_bitmap.view_bits_mut::<bitvec::prelude::Lsb0>();

        for (index, i) in bits.iter_mut().enumerate().skip(self.last_page_used) {
            if !*i {
                self.last_page_used = index;
                return Some(index);
            }
        }
        None
    }

    pub fn request_pages(&mut self, number: usize) -> Option<usize> {
        let pages = self.find_pages(number)?;
        self.lock_pages(pages, number);
        let pages = pages * 4096;

        // Zero pages
        unsafe { core::ptr::write_bytes(pages as *mut u64, 0, 4096 * number / 8) };

        Some(pages)
    }

    fn find_pages(&mut self, number: usize) -> Option<usize> {
        let bits = self.page_bitmap.view_bits_mut::<bitvec::prelude::Lsb0>();

        let mut start_frame = None;
        let mut pages = 0;

        for (index, i) in bits.iter_mut().enumerate() {
            // Is frame available
            if !*i {
                // Is this the first good frame after a chain of locked
                if let None = start_frame {
                    start_frame = Some(index);
                    pages = 0;
                };
                pages += 1;
                if pages == number {
                    return start_frame;
                }
            } else {
                start_frame = None;
            }
        }
        None
    }

    pub fn free_page(&mut self, page: usize) {
        let bits = self.page_bitmap.view_bits_mut::<bitvec::prelude::Lsb0>();
        let bit = bits.get_mut(page).unwrap();
        if *bit {
            bit.commit(false);
            self.last_page_used = page;
            self.free_pages += 1;
            self.used_pages -= 1;
        } else {
            println!("WARN: tried to free unallocated page: {}", page)
        }
    }

    pub fn free_pages(&mut self, page: usize, page_cnt: usize) {
        for i in 0..page_cnt {
            self.free_page(page + i);
        }
    }

    pub fn lock_page(&mut self, page: usize) {
        let bits = self.page_bitmap.view_bits_mut::<bitvec::prelude::Lsb0>();
        let bit = bits.get_mut(page).unwrap();
        if !*bit {
            bit.commit(true);
            self.free_pages -= 1;
            self.used_pages += 1;
        } else {
            println!("WARN: tried to lock allocated page: {}", page)
        }
    }

    pub fn lock_pages(&mut self, page: usize, page_cnt: usize) {
        for i in 0..page_cnt {
            self.lock_page(page + i);
        }
    }

    pub fn unreserve_page(&mut self, page: usize) {
        let bits = self.page_bitmap.view_bits_mut::<bitvec::prelude::Lsb0>();
        let bit = bits.get_mut(page).unwrap();
        if *bit {
            bit.commit(false);
            self.free_pages += 1;
            self.reserved_pages -= 1;
        } else {
            println!("WARN: tried to unreserve_page unallocated page: {}", page)
        }
    }

    pub fn unreserve_pages(&mut self, page: usize, page_cnt: usize) {
        for i in 0..page_cnt {
            self.unreserve_page(page + i);
        }
    }

    pub fn reserve_page(&mut self, page: usize) {
        let bits = self.page_bitmap.view_bits_mut::<bitvec::prelude::Lsb0>();
        let bit = bits.get_mut(page).unwrap();
        if !*bit {
            bit.commit(true);
            self.free_pages -= 1;
            self.reserved_pages += 1;
        } else {
            println!("WARN: tried to reserve allocated page: {}", page)
        }
    }

    pub fn reserve_pages(&mut self, page: usize, page_cnt: usize) {
        for i in 0..page_cnt {
            self.lock_page(page + i);
        }
    }
}
