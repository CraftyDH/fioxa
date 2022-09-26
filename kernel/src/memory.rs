use uefi::table::boot::MemoryDescriptor;

pub fn get_memory_size_pages(mmap: &[MemoryDescriptor]) -> u64 {
    let mut memory_size = 0;
    for md in mmap {
        memory_size += md.page_count
    }
    memory_size
}

