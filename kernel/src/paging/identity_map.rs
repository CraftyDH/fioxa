use crate::{memory::MemoryMapIter, screen::gop::WRITER};

use super::page_table_manager::PageTableManager;

pub fn identity_map(mapper: &mut PageTableManager, mmap: MemoryMapIter) {
    // for i in 0..get_memory_size_pages(mmap) + 1 {
    //     if i % 10000 == 0 {
    //         print!(".");
    //     }
    //     mapper.map_memory(i * 4096, i * 4096).unwrap().ignore();
    // }
    // Only map actual memory
    for r in mmap {
        print!(".");
        for i in (r.phys_start..(r.phys_start + r.page_count * 0x1000)).step_by(0x1000) {
            mapper.map_memory(i, i).unwrap().ignore();
        }
    }

    let fb_base = *WRITER.lock().gop.buffer.get_mut() as u64;
    let fb_size = fb_base + (WRITER.lock().gop.buffer_size as u64);

    for i in (fb_base..fb_size).step_by(0x1000) {
        mapper.map_memory(i, i).unwrap().ignore();
    }

    unsafe {
        core::arch::asm!("mov cr3, {}", in(reg) mapper.get_lvl4_addr(), options(nostack, preserves_flags))
    };
}
