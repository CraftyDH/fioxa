use elf::{
    abi::{PF_W, PT_LOAD},
    endian::NativeEndian,
};
use uefi::boot::{AllocateType, MemoryType, allocate_pages};

use crate::paging::page_table_manager::PageTableManager;

pub fn load_kernel(kernel_data: &[u8], mapper: &mut PageTableManager) -> u64 {
    let elf = elf::ElfBytes::<NativeEndian>::minimal_parse(kernel_data).unwrap();

    info!("Copying kernel");
    // Iterate over each header
    for program_header in elf.segments().unwrap() {
        if program_header.p_type == PT_LOAD {
            let base = program_header.p_vaddr & !0xFFF;
            let top = (program_header.p_vaddr + program_header.p_memsz + 0xFFF) & !0xFFF;
            let pcount = (top - base).div_ceil(0x1000);

            let pages = allocate_pages(
                AllocateType::AnyPages,
                MemoryType::LOADER_DATA,
                pcount as usize,
            )
            .unwrap();

            // zero range
            unsafe {
                core::ptr::write_bytes(pages.as_ptr() as *mut u8, 0, pcount as usize * 0x1000)
            }

            for i in 0..pcount {
                match mapper.map_memory(base + i * 0x1000, pages.as_ptr() as u64 + i * 0x1000, true)
                {
                    Ok(f) => f.flush(),
                    Err(e) => error!("{e:?}"),
                }
            }

            let data = elf.segment_data(&program_header).unwrap();

            unsafe {
                core::ptr::copy_nonoverlapping(
                    data.as_ptr(),
                    program_header.p_vaddr as *mut u8,
                    data.len(),
                )
            }

            if (program_header.p_flags & PF_W) == 0 {
                for i in 0..pcount {
                    match mapper.protect_memory(base + i * 0x1000) {
                        Ok(f) => f.flush(),
                        Err(e) => error!("{e:?}"),
                    }
                }
            }
        }
    }

    elf.ehdr.e_entry
}
