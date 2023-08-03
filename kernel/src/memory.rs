use bootloader::uefi::table::boot::MemoryDescriptor;

pub fn get_memory_size_pages(mmap: MemoryMapIter) -> u64 {
    let mut memory_size = 0;
    for md in mmap {
        memory_size += md.page_count
    }
    memory_size
}

/// An iterator of memory descriptors
/// Copied from uefi crate
#[derive(Debug, Clone)]
pub struct MemoryMapIter<'buf> {
    buffer: &'buf [u8],
    entry_size: usize,
    index: usize,
    len: usize,
}

impl<'buf> MemoryMapIter<'buf> {
    pub fn new(buffer: &'buf [u8], entry_size: usize, len: usize) -> Self {
        Self {
            buffer,
            entry_size,
            index: 0,
            len,
        }
    }
}

impl<'buf> Iterator for MemoryMapIter<'buf> {
    type Item = &'buf MemoryDescriptor;

    fn size_hint(&self) -> (usize, Option<usize>) {
        let sz = self.len - self.index;

        (sz, Some(sz))
    }

    fn next(&mut self) -> Option<Self::Item> {
        if self.index < self.len {
            let ptr = self.buffer.as_ptr() as usize + self.entry_size * self.index;

            self.index += 1;

            let descriptor = unsafe { &*(ptr as *const MemoryDescriptor) };

            Some(descriptor)
        } else {
            None
        }
    }
}
