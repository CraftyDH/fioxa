use bootloader::{MemoryClass, MemoryMapEntry, MemoryMapEntrySlice, MemoryMapEntrySliceIter};

use crate::paging::{
    page_table_manager::{Page, Size4KB},
    virt_addr_offset,
};

pub const RESERVED_32BIT_MEM_PAGES: u64 = 32; // 16Kb

pub fn get_memory_size_pages(mmap: MemoryMapEntrySlice) -> u64 {
    let mut memory_size = 0;
    for md in mmap.iter() {
        memory_size += unsafe { &*virt_addr_offset(md) }.page_count
    }
    memory_size
}

#[derive(Debug, Clone)]
pub struct MemoryDescriptorPageIter {
    descriptor: *const MemoryMapEntry,
    index: u64,
}

impl From<*const MemoryMapEntry> for MemoryDescriptorPageIter {
    fn from(value: *const MemoryMapEntry) -> Self {
        Self {
            descriptor: value,
            index: 0,
        }
    }
}

impl Iterator for MemoryDescriptorPageIter {
    type Item = Page<Size4KB>;

    fn size_hint(&self) -> (usize, Option<usize>) {
        (self.len(), Some(self.len()))
    }

    fn next(&mut self) -> Option<Self::Item> {
        let desc = unsafe { &*virt_addr_offset(self.descriptor) };
        if self.index < desc.page_count {
            let res = self.index;
            self.index += 1;
            Some(Page::new(desc.phys_start + res * 0x1000))
        } else {
            None
        }
    }
}

impl ExactSizeIterator for MemoryDescriptorPageIter {
    fn len(&self) -> usize {
        let desc = unsafe { &*virt_addr_offset(self.descriptor) };
        (desc.page_count - self.index) as usize
    }
}

#[derive(Debug, Clone)]
pub struct BootPageAllocator {
    map: MemoryMapEntrySliceIter,
    desc: Option<MemoryDescriptorPageIter>,
    u32_bits_reserved: bool,
}

unsafe impl Send for BootPageAllocator {}

impl From<MemoryMapEntrySliceIter> for BootPageAllocator {
    fn from(value: MemoryMapEntrySliceIter) -> Self {
        Self {
            map: value,
            desc: None,
            u32_bits_reserved: false,
        }
    }
}

impl Iterator for BootPageAllocator {
    type Item = Page<Size4KB>;

    fn next(&mut self) -> Option<Self::Item> {
        loop {
            match &mut self.desc {
                Some(desc) => match desc.next() {
                    Some(page) => return Some(page),
                    None => self.desc = None,
                },
                None => {
                    self.desc = Some({
                        loop {
                            let next = self.map.next()?;
                            let n = unsafe { &*virt_addr_offset(next) };
                            if n.class == MemoryClass::Free {
                                if !self.u32_bits_reserved {
                                    // these things to skip are important as the proper allocator assumes they will not have been allocated
                                    if n.phys_start <= u16::MAX.into() {
                                        continue;
                                    } else if n.page_count >= RESERVED_32BIT_MEM_PAGES {
                                        assert!(n.phys_start <= u32::MAX.into());
                                        self.u32_bits_reserved = true;
                                        continue;
                                    }
                                }
                                break next.into();
                            }
                        }
                    })
                }
            }
        }
    }
}
