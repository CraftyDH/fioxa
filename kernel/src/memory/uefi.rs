use spin::mutex::Mutex;
use uefi::table::boot::{MemoryDescriptor, MemoryType};
use x86_64::{
    structures::paging::{
        mapper::MapToError, FrameAllocator, Mapper, OffsetPageTable, Page, PageTable,
        PageTableFlags, PhysFrame, Size2MiB, Size4KiB,
    },
    PhysAddr, VirtAddr,
};

use crate::screen::gop;

pub fn get_memory_size(mmap: &[MemoryDescriptor]) -> usize {
    let mut page_count = 0;
    for entry in mmap {
        // if entry.ty == MemoryType::CONVENTIONAL
        //     || entry.ty == MemoryType::LOADER_CODE
        //     || entry.ty == MemoryType::LOADER_DATA
        // {
        page_count += entry.page_count as usize * 4096
        // }
    }

    page_count
}

pub struct UEFIFrameAllocator<'a> {
    memory_map: Option<&'a [MemoryDescriptor]>,
    next: usize,
}

impl<'a> UEFIFrameAllocator<'a> {
    pub fn init(&mut self, memory_map: &'a [MemoryDescriptor]) {
        self.memory_map = Some(memory_map.clone())
    }

    fn usable_frames(&self) -> impl Iterator<Item = PhysFrame> + 'a {
        if let Some(memory_map) = self.memory_map {
            // Get all regions from memory map
            let regions = memory_map.clone().iter();

            // Filter out memory regions that aren't usable
            let usable_regions = regions.filter(|r| r.ty == MemoryType::CONVENTIONAL);

            // Create iterator of the range of available regions
            let addr_ranges =
                usable_regions.map(|r| r.phys_start..(r.phys_start + r.page_count * 4096));

            // Transform to an iterator of frame start addresses.
            let frame_addresses = addr_ranges.flat_map(|r| r.step_by(4096));

            frame_addresses.map(|addr| PhysFrame::containing_address(PhysAddr::new(addr)))
        } else {
            panic!("UEFI frame allocator used before init")
        }
    }
}

unsafe impl<'a> FrameAllocator<Size4KiB> for UEFIFrameAllocator<'a> {
    fn allocate_frame(&mut self) -> Option<PhysFrame<Size4KiB>> {
        let frame = self.usable_frames().nth(self.next);
        self.next += 1;

        if let Some(fr) = frame {
            unsafe {
                core::ptr::write_bytes(
                    fr.start_address().as_u64() as *mut PhysFrame<Size4KiB>,
                    0,
                    0x1000,
                );
            }
        }

        frame
    }
}

pub static FRAME_ALLOCATOR: Mutex<UEFIFrameAllocator> = Mutex::new(UEFIFrameAllocator {
    memory_map: None,
    next: 1000,
});

pub unsafe fn identity_map_all_memory<'a>(
    // frame_allocator: &mut impl FrameAllocator<Size4KiB>,
    mmap: &'a [MemoryDescriptor],
) -> OffsetPageTable<'a> {
    // UEFI Identity maps all memory
    // So we will do the same (for now)
    let phys_offset = VirtAddr::new(0);

    // Copy current page
    let _bootloader_page_table = {
        let old_table = {
            let frame = x86_64::registers::control::Cr3::read().0;
            let ptr: *const PageTable = (phys_offset + frame.start_address().as_u64()).as_ptr();
            &*ptr
        };
        let new_frame = FRAME_ALLOCATOR
            .lock()
            .allocate_frame()
            .expect("Failed to allocate frame for new level 4 table");
        let new_table: &mut PageTable = {
            let ptr: *mut PageTable =
                (phys_offset + new_frame.start_address().as_u64()).as_mut_ptr();
            // create a new, empty page table
            ptr.write(PageTable::new());
            &mut *ptr
        };

        // copy the first entry (we don't need to access more than 512 GiB; also, some UEFI
        // implementations seem to create an level 4 table entry 0 in all slots)
        new_table[0] = old_table[0].clone();

        // the first level 4 table entry is now identical, so we can just load the new one
        x86_64::registers::control::Cr3::write(
            new_frame,
            x86_64::registers::control::Cr3Flags::empty(),
        );
        OffsetPageTable::new(&mut *new_table, phys_offset)
    };

    // Get a new frame for the top level mapping
    let new_map_ptr = FRAME_ALLOCATOR.lock().allocate_frame().unwrap();

    // Get the address of this new memory block
    let addr: *mut PageTable = (phys_offset + new_map_ptr.start_address().as_u64()).as_mut_ptr();

    // Write a new page table to it
    *addr = PageTable::new();

    let mut new_mapper = OffsetPageTable::new(&mut *addr, phys_offset);

    print!("mem_size: {:?}", get_memory_size(mmap));
    // for i in (0..(get_memory_size(mmap)) - 0x1000).step_by(0x1000) {
    //     // if i > 850000000 * 4096 {
    //     //     continue;
    //     // }
    //     // print!(" |{:#X}|", i);
    //     new_mapper
    //         .identity_map(
    //             PhysFrame::<Size4KiB>::containing_address(PhysAddr::new(i as u64)),
    //             PageTableFlags::WRITABLE | PageTableFlags::PRESENT,
    //             frame_allocator,
    //         )
    //         .unwrap()
    //         .ignore();
    // }

    let start_frame = PhysFrame::containing_address(PhysAddr::new(0));
    let max_phys = PhysAddr::new(get_memory_size(mmap) as u64 + 0x1000);
    let end_frame: PhysFrame<Size4KiB> = PhysFrame::containing_address(max_phys);
    for frame in PhysFrame::range_inclusive(start_frame, end_frame) {
        let page = Page::containing_address(phys_offset + frame.start_address().as_u64());
        let flags = PageTableFlags::PRESENT | PageTableFlags::WRITABLE;
        match new_mapper.map_to(page, frame, flags, &mut *FRAME_ALLOCATOR.lock()) {
            Ok(tlb) => tlb.ignore(),
            Err(err) => panic!(
                "failed to map page {:?} to frame {:?}: {:?}",
                page, frame, err
            ),
        };
    }

    // Get the base and size of the GOP buffer
    let buffer_size = gop::WRITER.lock().gop.buffer_size;
    let buffer_base = gop::WRITER
        .lock()
        .gop
        .buffer
        .load(core::sync::atomic::Ordering::Relaxed) as usize;

    let framebuffer_start_frame =
        PhysFrame::<Size2MiB>::containing_address(PhysAddr::new(buffer_base as u64));
    let framebuffer_end_frame =
        PhysFrame::containing_address(PhysAddr::new((buffer_base + buffer_size - 1) as u64));
    let start_page = Page::<Size2MiB>::containing_address(VirtAddr::new(buffer_base as u64));

    for (i, frame) in
        PhysFrame::<Size2MiB>::range_inclusive(framebuffer_start_frame, framebuffer_end_frame)
            .enumerate()
    {
        let page = start_page + i as u64;
        let flags = PageTableFlags::PRESENT | PageTableFlags::WRITABLE;
        match new_mapper.map_to(page, frame, flags, &mut *FRAME_ALLOCATOR.lock()) {
            Ok(tlb) => tlb.flush(),
            // Its fine if allready mapped
            Err(MapToError::PageAlreadyMapped(_)) => (),
            Err(err) => panic!(
                "failed to map page {:?} to frame {:?}: {:?}",
                page, frame, err
            ),
        }
    }

    println!("!Success!");

    // Set this new mapping as the mapping for the kernel
    x86_64::registers::control::Cr3::write(
        new_map_ptr,
        x86_64::registers::control::Cr3Flags::empty(),
    );
    new_mapper
}

// pub const FRAMEALLOCATOR: OnceCell<Mutex<UEFIFrameAllocator>> = OnceCell::uninit();
