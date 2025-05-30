use alloc::sync::Arc;
use bootloader::{BootInfo, gop::GopInfo};
use spin::Once;

use crate::{
    kernel_memory_loc,
    memory::MemoryMapIter,
    paging::{
        MemoryLoc,
        page::{Page, get_chunked_page_range},
        page_table::Mapper,
        virt_addr_offset,
    },
};

use super::{
    MemoryMappingFlags, PageAllocator,
    page::Size4KB,
    page_mapper::PageMapping,
    page_table::{PageTable, TableLevel3, TableLevel4},
};

pub unsafe fn create_offset_map(
    alloc: &impl PageAllocator,
    mapper: &mut PageTable<TableLevel3>,
    mmap: MemoryMapIter,
) {
    // Only map actual memory
    // This means we will get a page fault on access to a non ram in the offset table
    // (instead of accessing memory holes/complely non existend addresses)
    for r in mmap {
        let r = unsafe { &*virt_addr_offset(r) };

        assert!(
            r.phys_start + r.page_count * 1000
                <= MemoryLoc::_EndPhysMapOffset as u64 - MemoryLoc::PhysMapOffset as u64
        );

        // println!("{:?}", r);
        let pages = get_chunked_page_range(
            r.phys_start.max(0x1000), // Ignore the zero page for now
            r.phys_start + r.page_count * 0x1000,
        );

        for page in [pages.0, pages.4].into_iter() {
            for page in page.into_iter() {
                mapper
                    .map(
                        alloc,
                        Page::new(page.get_address() + MemoryLoc::PhysMapOffset as u64),
                        page,
                        MemoryMappingFlags::WRITEABLE,
                    )
                    .unwrap()
                    .ignore();
            }
        }

        for page in [pages.1, pages.3].into_iter() {
            for page in page.into_iter() {
                mapper
                    .map(
                        alloc,
                        Page::new(page.get_address() + MemoryLoc::PhysMapOffset as u64),
                        page,
                        MemoryMappingFlags::WRITEABLE,
                    )
                    .unwrap()
                    .ignore();
            }
        }

        for page in pages.2 {
            mapper
                .map(
                    alloc,
                    Page::new(page.get_address() + MemoryLoc::PhysMapOffset as u64),
                    page,
                    MemoryMappingFlags::WRITEABLE,
                )
                .unwrap()
                .ignore();
        }
    }
}

pub unsafe fn map_gop(
    alloc: &impl PageAllocator,
    mapper: &mut PageTable<TableLevel4>,
    gop: &GopInfo,
) {
    // Map GOP framebuffer
    let fb_base = unsafe { *gop.buffer.as_ptr() as u64 };
    let fb_size = fb_base + (gop.buffer_size as u64);

    for i in (fb_base..fb_size + 0xFFF).step_by(0x1000) {
        mapper
            .identity_map(
                alloc,
                Page::<Size4KB>::new(i),
                MemoryMappingFlags::WRITEABLE,
            )
            .unwrap()
            .ignore();
    }
}

pub unsafe fn get_gop_range(gop: &GopInfo) -> (usize, Arc<PageMapping>) {
    static GOP_RANGE: Once<(usize, Arc<PageMapping>)> = Once::new();
    GOP_RANGE
        .call_once(|| unsafe {
            let fb_ptr = *gop.buffer.as_ptr() as usize;

            let fb_base = fb_ptr & !0xFFF;

            let fb_top = (fb_base + gop.buffer_size as usize + 0xFFF) & !0xFFF;

            (fb_base, PageMapping::new_mmap(fb_base, fb_top - fb_base))
        })
        .clone()
}

pub unsafe fn create_kernel_map(
    alloc: &impl PageAllocator,
    mapper: &mut PageTable<TableLevel3>,
    boot_info: &BootInfo,
) {
    // Map ourself
    let base = boot_info.kernel_start;
    let pages = boot_info.kernel_pages;
    let (kern_base, _) = kernel_memory_loc();

    assert!(kern_base == MemoryLoc::KernelStart as u64);
    for i in (0..pages * 0x1000).step_by(0x1000) {
        mapper
            .map(
                alloc,
                Page::<Size4KB>::new(MemoryLoc::KernelStart as u64 + i),
                Page::<Size4KB>::new(base + i),
                MemoryMappingFlags::WRITEABLE,
            )
            .unwrap()
            .ignore();
    }
}
