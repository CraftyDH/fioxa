use bootloader::uefi::table::boot::{MemoryDescriptor, MemoryType};

use crate::paging::{
    page_table_manager::{Page, Size4KB},
    virt_addr_offset,
};

pub const RESERVED_32BIT_MEM_PAGES: u64 = 32; // 16Kb

pub fn get_memory_size_pages(mmap: MemoryMapIter) -> u64 {
    let mut memory_size = 0;
    for md in mmap {
        memory_size += unsafe { &*virt_addr_offset(md) }.page_count
    }
    memory_size
}

/// An iterator of memory descriptors
/// Copied from uefi crate
#[derive(Debug, Clone)]
pub struct MemoryMapIter {
    buffer: *const u8,
    entry_size: usize,
    index: usize,
    len: usize,
}

impl<'buf> MemoryMapIter {
    pub unsafe fn new(buffer: *const u8, entry_size: usize, len: usize) -> Self {
        Self {
            buffer,
            entry_size,
            index: 0,
            len,
        }
    }
}

impl Iterator for MemoryMapIter {
    type Item = *const MemoryDescriptor;

    fn size_hint(&self) -> (usize, Option<usize>) {
        (self.len(), Some(self.len()))
    }

    fn next(&mut self) -> Option<Self::Item> {
        if self.index < self.len {
            let ptr = unsafe { self.buffer.add(self.entry_size * self.index) };

            self.index += 1;

            Some(ptr as *const MemoryDescriptor)
        } else {
            None
        }
    }
}

impl ExactSizeIterator for MemoryMapIter {
    fn len(&self) -> usize {
        self.len - self.index
    }
}

#[derive(Debug, Clone)]
pub struct MemoryMapUsuableIter {
    map: MemoryMapIter,
    u32_bits_reserved: bool,
}

impl From<MemoryMapIter> for MemoryMapUsuableIter {
    fn from(value: MemoryMapIter) -> Self {
        Self {
            map: value,
            u32_bits_reserved: false,
        }
    }
}

impl Iterator for MemoryMapUsuableIter {
    type Item = *const MemoryDescriptor;

    fn size_hint(&self) -> (usize, Option<usize>) {
        (0, Some(self.map.len()))
    }

    fn next(&mut self) -> Option<Self::Item> {
        loop {
            let res = self.map.next()?;
            let val = unsafe { &*(virt_addr_offset(res)) };
            if val.ty == MemoryType::CONVENTIONAL
                || val.ty == MemoryType::BOOT_SERVICES_CODE
                || val.ty == MemoryType::BOOT_SERVICES_DATA
            {
                if !self.u32_bits_reserved {
                    // these things to skip are important as the proper allocator assumes they will not have been allocated
                    if val.phys_start <= u16::MAX.into() {
                        continue;
                    } else if val.page_count >= RESERVED_32BIT_MEM_PAGES {
                        assert!(val.phys_start <= u32::MAX.into());
                        self.u32_bits_reserved = true;
                        continue;
                    } else {
                        return Some(res);
                    }
                }
                return Some(res);
            }
        }
    }
}

#[derive(Debug, Clone)]
pub struct MemoryDescriptorPageIter {
    descriptor: *const MemoryDescriptor,
    index: u64,
}

impl From<*const MemoryDescriptor> for MemoryDescriptorPageIter {
    fn from(value: *const MemoryDescriptor) -> Self {
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
pub struct MemoryMapPageIter {
    memmap: MemoryMapUsuableIter,
    desc: Option<MemoryDescriptorPageIter>,
}

unsafe impl Send for MemoryMapPageIter {}

impl From<MemoryMapUsuableIter> for MemoryMapPageIter {
    fn from(value: MemoryMapUsuableIter) -> Self {
        Self {
            memmap: value,
            desc: None,
        }
    }
}

impl Iterator for MemoryMapPageIter {
    type Item = Page<Size4KB>;

    fn next(&mut self) -> Option<Self::Item> {
        loop {
            match &mut self.desc {
                Some(desc) => match desc.next() {
                    Some(page) => return Some(page),
                    None => self.desc = None,
                },
                None => self.desc = Some(self.memmap.next()?.into()),
            }
        }
    }
}
