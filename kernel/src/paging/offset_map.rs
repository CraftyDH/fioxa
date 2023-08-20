use crate::{
    kernel_memory_loc,
    memory::MemoryMapIter,
    paging::{
        page_table_manager::{get_chunked_page_range, Mapper, Page},
        MemoryLoc,
    },
    screen::gop::WRITER,
    BOOT_INFO,
};

use super::page_table_manager::{PageLvl3, PageLvl4, PageTable, Size4KB};

pub fn create_offset_map(mapper: &mut PageTable<PageLvl3>, mmap: MemoryMapIter) {
    // Only map actual memory
    // This means we will get a page fault on access to a non ram in the offset table
    // (instead of accessing memory holes/complely non existend addresses)
    for r in mmap {
        print!(".");

        // println!("{:?}", r);
        let pages = get_chunked_page_range(r.phys_start, r.phys_start + r.page_count * 0x1000);

        for page in [pages.0, pages.4].into_iter() {
            for page in page.into_iter() {
                mapper
                    .map_memory(
                        Page::new(page.get_address() + MemoryLoc::PhysMapOffset as u64),
                        page,
                    )
                    .unwrap()
                    .ignore();
            }
        }

        for page in [pages.1, pages.3].into_iter() {
            for page in page.into_iter() {
                mapper
                    .map_memory(
                        Page::new(page.get_address() + MemoryLoc::PhysMapOffset as u64),
                        page,
                    )
                    .unwrap()
                    .ignore();
            }
        }

        for page in pages.2 {
            mapper
                .map_memory(
                    Page::new(page.get_address() + MemoryLoc::PhysMapOffset as u64),
                    page,
                )
                .unwrap()
                .ignore();
        }

        // for i in (r.phys_start..(r.phys_start + r.page_count * 0x1000)).step_by(0x1000) {
        //     mapper
        //         .map_memory(page_4kb(MemoryLoc::PhysMapOffset as u64 + i), page_4kb(i))
        //         .unwrap()
        //         .ignore();
        // }
    }
}

pub fn map_gop(mapper: &mut PageTable<PageLvl4>) {
    // Map GOP framebuffer
    let fb_base = *WRITER.lock().gop.buffer.get_mut() as u64;
    let fb_size = fb_base + (WRITER.lock().gop.buffer_size as u64);

    for i in (fb_base..fb_size + 0xFFF).step_by(0x1000) {
        mapper
            .identity_map_memory(Page::<Size4KB>::new(i))
            .unwrap()
            .ignore();
    }
}

pub fn create_kernel_map(mapper: &mut PageTable<PageLvl3>) {
    let boot_info = unsafe { &*BOOT_INFO };

    // Map ourself
    let base = boot_info.kernel_start;
    let pages = boot_info.kernel_pages;
    let (kern_base, _) = kernel_memory_loc();

    assert!(kern_base == MemoryLoc::KernelStart as u64);
    for i in (0..pages * 0x1000).step_by(0x1000) {
        mapper
            .map_memory(
                Page::<Size4KB>::new(MemoryLoc::KernelStart as u64 + i),
                Page::<Size4KB>::new(base + i),
            )
            .unwrap()
            .ignore();
    }
}
