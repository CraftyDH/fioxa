use uefi::table::boot::MemoryDescriptor;

use crate::{memory::get_memory_size_pages, screen::gop::WRITER};

use super::page_table_manager::PageTableManager;

pub fn identity_map(mapper: &mut PageTableManager, mmap: &[MemoryDescriptor]) {
    for i in 0..get_memory_size_pages(mmap) + 1 {
        if i % 10000 == 0 {
            print!(".");
        }
        mapper.map_memory(i * 4096, i * 4096);
    }

    let fb_base = *WRITER.lock().gop.buffer.get_mut() as u64 / 4096;
    let fb_size = fb_base + (WRITER.lock().gop.buffer_size as u64 / 4096);

    for i in fb_base..fb_size {
        mapper.map_memory(i * 4096, i * 4096);
    }

    unsafe {
        core::arch::asm!("mov cr3, {}", in(reg) mapper.get_lvl4_addr(), options(nostack, preserves_flags))
    };
}
