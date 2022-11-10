use spin::Mutex;

use crate::{memory::MemoryMapIter, paging::page_allocator::request_page, screen::gop::WRITER};

use super::page_table_manager::PageTableManager;

#[no_mangle]
pub static mut pml4_ptr: u64 = 0;

lazy_static::lazy_static! {
    pub static ref FULL_IDENTITY_MAP: Mutex<PageTableManager> =
        Mutex::new({
            let page = request_page().unwrap();
            PageTableManager::new(page)
        });
}

pub fn create_full_identity_map(mmap: MemoryMapIter) {
    let mut mapper = FULL_IDENTITY_MAP.lock();
    // Only map actual memory
    for r in mmap {
        print!(".");
        for i in (r.phys_start..(r.phys_start + r.page_count * 0x1000)).step_by(0x1000) {
            mapper.map_memory(i, i, true).unwrap().ignore();
        }
    }

    // Map GOP framebuffer
    let fb_base = *WRITER.lock().gop.buffer.get_mut() as u64;
    let fb_size = fb_base + (WRITER.lock().gop.buffer_size as u64);

    for i in (fb_base..fb_size + 0x1000).step_by(0x1000) {
        mapper.map_memory(i, i, true).unwrap().ignore();
    }

    // Set ptr so BSP's can load it when they are booted later
    unsafe { pml4_ptr = mapper.get_lvl4_addr() }
}
