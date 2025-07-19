use core::cmp::Ordering;

use alloc::{boxed::Box, sync::Arc, vec::Vec};
use kernel_sys::types::{VMMapFlags, VMOAnonymousFlags};

use crate::{
    mutex::Spinlock,
    paging::{
        MemoryLoc, PageAllocator,
        page::{Page, Size4KB},
        page_allocator::{frame_alloc_exec, global_allocator},
        page_table::{Mapper, PageTable, TableLevel4, UnMapMemoryError},
    },
};

pub struct VirtualMemoryRegion {
    page_mapper: PageTable<TableLevel4>,
    // Should always be ordered
    mappings: Vec<VMOMapping>,
}

struct VMOMapping {
    backing: Arc<Spinlock<VMO>>,
    map_flags: VMMapFlags,
    base_vaddr: usize,
    end_vaddr: usize,
}

impl VirtualMemoryRegion {
    pub fn new(alloc: &impl PageAllocator) -> Self {
        Self {
            page_mapper: PageTable::new_with_global(alloc),
            mappings: Vec::new(),
        }
    }

    pub fn get_cr3(&self) -> usize {
        self.page_mapper.get_physical_address()
    }

    pub fn get_phys_addr_from_vaddr(&self, address: u64) -> Option<u64> {
        self.page_mapper.get_phys_addr_from_vaddr(address)
    }

    pub fn map_vmo(
        &mut self,
        vmo: Arc<Spinlock<VMO>>,
        flags: VMMapFlags,
        hint: Option<usize>,
    ) -> Result<usize, MapVMOError> {
        let vmo_locked = vmo.lock();

        let length = vmo_locked.get_length();

        let base = match hint {
            Some(val) => {
                if val & 0xFFF != 0 || val + length > MemoryLoc::EndUserMem as usize {
                    // unaligned
                    return Err(MapVMOError::BadHint);
                }

                for m in &self.mappings {
                    if val < m.end_vaddr && m.base_vaddr <= val + length {
                        return Err(MapVMOError::AddressUsed);
                    }
                }

                val
            }
            None => self
                .find_free_addr(length)
                .ok_or(MapVMOError::NoFreeRange)?,
        };

        let alloc = global_allocator();

        let mut map = |vaddr, page| {
            self.page_mapper
                .map(alloc, Page::<Size4KB>::new(vaddr as u64), page, flags)
                .unwrap()
                .ignore();
        };

        let virt = (base..base + length).step_by(0x1000);

        match &*vmo_locked {
            VMO::MemoryMapped {
                base_address,
                length,
            } => {
                (*base_address..base_address + length)
                    .step_by(0x1000)
                    .zip(virt)
                    .for_each(|(p, v)| map(v, Page::new(p as u64)));
            }
            VMO::Anonymous { pages, .. } => pages
                .iter()
                .zip(virt)
                .filter_map(|(a, i)| a.map(|p| (p, i)))
                .for_each(|(p, v)| map(v, p)),
        };

        let idx = self
            .mappings
            .binary_search_by_key(&base, |m| m.base_vaddr)
            .unwrap_err();

        drop(vmo_locked);

        self.mappings.insert(
            idx,
            VMOMapping {
                backing: vmo,
                map_flags: flags,
                base_vaddr: base,
                end_vaddr: base + length,
            },
        );

        Ok(base)
    }

    pub fn page_fault_handler(&mut self, address: usize) -> Option<()> {
        let idx = self.find_region_by_address(address)?;
        let map = &mut self.mappings[idx];
        let offset = address - map.base_vaddr;
        let phys = match &mut *map.backing.lock() {
            VMO::MemoryMapped { base_address, .. } => {
                Page::containing((*base_address + offset) as u64)
            }
            VMO::Anonymous { flags, pages } => {
                let idx = offset / 0x1000;
                let page = &mut pages[idx];
                match page {
                    Some(p) => *p,
                    None => {
                        let p = if flags.contains(VMOAnonymousFlags::BELOW_32) {
                            frame_alloc_exec(|a| a.allocate_page_32bit())
                        } else {
                            global_allocator().allocate_page()
                        };
                        let p = p.unwrap();
                        *page = Some(p);
                        p
                    }
                }
            }
        };
        // Make the mapping
        if let Ok(f) = self.page_mapper.map(
            global_allocator(),
            Page::<Size4KB>::containing(address as u64),
            phys,
            map.map_flags,
        ) {
            f.flush()
        }
        Some(())
    }

    fn find_region_by_address(&self, address: usize) -> Option<usize> {
        self.mappings
            .binary_search_by(|m| {
                if m.base_vaddr > address {
                    Ordering::Greater
                } else if m.end_vaddr <= address {
                    Ordering::Less
                } else {
                    Ordering::Equal
                }
            })
            .ok()
    }

    fn find_free_addr(&self, length: usize) -> Option<usize> {
        let ranges = core::iter::once((0usize, 0x1000usize))
            .chain(self.mappings.iter().map(|m| (m.base_vaddr, m.end_vaddr)))
            .chain(core::iter::once((
                MemoryLoc::EndUserMem as usize,
                usize::MAX,
            )));

        let free = ranges.map_windows(|[left, right]| (left.1, right.0 - left.1));

        free.filter(|r| r.1 >= length).map(|r| r.0).next()
    }

    pub unsafe fn unmap(&mut self, address: usize, length: usize) -> Result<(), UnmapError> {
        let idx = self
            .find_region_by_address(address)
            .ok_or(UnmapError::NotMapped)?;

        let map = self.mappings.remove(idx);

        let length = (length + 0xFFF) & !0xFFF;

        if map.base_vaddr != address || map.end_vaddr != address + length {
            return Err(UnmapError::MustUnmapVMOCompletely);
        }

        let alloc = global_allocator();
        for page in (map.base_vaddr..map.end_vaddr).step_by(0x1000) {
            match self
                .page_mapper
                .unmap(alloc, Page::<Size4KB>::new(page as u64))
            {
                Ok(f) => f.flush(),
                Err(UnMapMemoryError::MemNotMapped(_)) => (),
                Err(e) => {
                    error!("Failed to unmap because: {e:?}")
                }
            }

            // TODO: Send IPI to flush on other threads
        }
        Ok(())
    }
}

#[derive(Debug, Clone, Copy)]
pub enum MapVMOError {
    AddressUsed,
    BadHint,
    PermissionsNotAllowed,
    NoFreeRange,
}

#[derive(Debug, Clone, Copy)]
pub enum UnmapError {
    NotMapped,
    MustUnmapVMOCompletely,
}

pub enum VMO {
    MemoryMapped {
        base_address: usize,
        length: usize,
    },
    Anonymous {
        flags: VMOAnonymousFlags,
        pages: Box<[Option<Page<Size4KB>>]>,
    },
}

impl VMO {
    pub fn get_length(&self) -> usize {
        match self {
            VMO::MemoryMapped { length, .. } => *length,
            VMO::Anonymous { pages, .. } => pages.len() * 0x1000,
        }
    }

    pub unsafe fn new_mmap(base_address: usize, length: usize) -> Self {
        assert_eq!(base_address & 0xFFF, 0);
        assert_eq!(length & 0xFFF, 0);
        Self::MemoryMapped {
            base_address,
            length,
        }
    }

    pub fn new_anonymous(length: usize, flags: VMOAnonymousFlags) -> Self {
        let page_count = length.div_ceil(0x1000);

        let pages = if flags.contains(VMOAnonymousFlags::CONTINUOUS | VMOAnonymousFlags::BELOW_32) {
            todo!("Handle 32bit continuous pages");
        } else if flags.contains(VMOAnonymousFlags::CONTINUOUS) {
            let base = global_allocator().allocate_pages(page_count).unwrap();
            (0..page_count)
                .map(|i| Some(Page::<Size4KB>::new(base.get_address() + i as u64 * 0x1000)))
                .collect()
        } else if flags.contains(VMOAnonymousFlags::PINNED | VMOAnonymousFlags::BELOW_32) {
            (0..page_count)
                .map(|_| Some(frame_alloc_exec(|a| a.allocate_page_32bit()).unwrap()))
                .collect()
        } else if flags.contains(VMOAnonymousFlags::PINNED) {
            let alloc = global_allocator();
            (0..page_count)
                .map(|_| Some(alloc.allocate_page().unwrap()))
                .collect()
        } else {
            (0..page_count).map(|_| None).collect()
        };

        Self::Anonymous { flags, pages }
    }

    pub fn vmo_pages(&self) -> &[Option<Page<Size4KB>>] {
        match self {
            VMO::MemoryMapped { .. } => panic!("only for anon"),
            VMO::Anonymous { pages, .. } => pages,
        }
    }
}

impl Drop for VMO {
    fn drop(&mut self) {
        match self {
            VMO::MemoryMapped { .. } => (),
            VMO::Anonymous { flags, pages } => unsafe {
                if pages.is_empty() {
                    return;
                }
                if flags.contains(VMOAnonymousFlags::CONTINUOUS | VMOAnonymousFlags::BELOW_32) {
                    todo!("Handle 32bit continuous pages");
                } else if flags.contains(VMOAnonymousFlags::CONTINUOUS) {
                    // We know that we allocated a continuous block from the start
                    global_allocator().free_pages(pages.first().unwrap().unwrap(), pages.len());
                } else if flags.contains(VMOAnonymousFlags::BELOW_32) {
                    pages
                        .iter()
                        .filter_map(|p| *p)
                        .for_each(|p| frame_alloc_exec(|a| a.free_32bit_reserved_page(p)));
                } else {
                    let alloc = global_allocator();
                    pages
                        .iter()
                        .filter_map(|p| *p)
                        .for_each(|p| alloc.free_page(p));
                }
            },
        }
    }
}
