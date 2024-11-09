use core::{cmp::Ordering, fmt::Debug, mem::MaybeUninit, ops::Range};

use alloc::{boxed::Box, sync::Arc, vec::Vec};
use spin::Mutex;

use super::{
    page_allocator::{frame_alloc_exec, request_page, AllocatedPage},
    page_table_manager::{Mapper, Page, PageLvl4, PageTable, Size4KB, UnMapMemoryError},
    MemoryLoc, MemoryMappingFlags,
};

pub struct PageMapperManager<'a> {
    page_mapper: PageTable<'a, PageLvl4>,
    // start offset, end offset, mapping
    // this should always be ordered
    mappings: Vec<(Range<usize>, Arc<PageMapping>, MemoryMappingFlags)>,
}

#[derive(Debug)]
pub struct PageMapping {
    size: usize,
    mapping: PageMappingType,
}

impl PageMapping {
    pub fn size(&self) -> usize {
        self.size
    }

    pub fn base_top_stack(&self) -> usize {
        match &self.mapping {
            PageMappingType::LazyMapping { pages } => {
                let mut pages = pages.lock();
                let page = pages.last_mut().unwrap();
                page.0
                    .unwrap_or_else(|| {
                        let apage = request_page().unwrap();
                        let p = *apage;
                        page.set(apage);
                        p
                    })
                    .get_address() as usize
            }
            _ => panic!(),
        }
    }
}

pub enum PageMappingType {
    MMAP {
        base_address: usize,
    },
    LazyMapping {
        pages: Mutex<Box<[MaybeAllocatedPage]>>,
    },
}

impl Debug for PageMappingType {
    fn fmt(&self, f: &mut core::fmt::Formatter<'_>) -> core::fmt::Result {
        match self {
            Self::MMAP { base_address: _ } => f.debug_struct("MMAP").finish(),
            Self::LazyMapping { pages: _ } => f.debug_struct("LazyMapping").finish(),
        }
    }
}

impl PageMapping {
    pub fn new_lazy(size: usize) -> Arc<PageMapping> {
        let b = unsafe {
            let mut b = Box::new_uninit_slice((size + 0xFFF) / 0x1000);
            b.fill_with(|| MaybeUninit::new(MaybeAllocatedPage::new()));
            b.assume_init()
        };
        Arc::new(PageMapping {
            size,
            mapping: PageMappingType::LazyMapping { pages: b.into() },
        })
    }

    pub fn new_lazy_filled(size: usize) -> Arc<PageMapping> {
        let b = unsafe {
            let mut b = Box::new_uninit_slice((size + 0xFFF) / 0x1000);
            b.fill_with(|| MaybeUninit::new(request_page().unwrap().into()));
            b.assume_init()
        };
        Arc::new(PageMapping {
            size,
            mapping: PageMappingType::LazyMapping { pages: b.into() },
        })
    }

    pub fn new_lazy_prealloc(pages: Box<[MaybeAllocatedPage]>) -> Arc<Self> {
        Arc::new(Self {
            size: pages.len() * 0x1000,
            mapping: PageMappingType::LazyMapping {
                pages: pages.into(),
            },
        })
    }

    pub unsafe fn new_mmap(base_address: usize, size: usize) -> Arc<Self> {
        assert_eq!(base_address & 0xFFF, 0);
        assert_eq!(size & 0xFFF, 0);
        Arc::new(Self {
            size,
            mapping: PageMappingType::MMAP { base_address },
        })
    }
}

impl<'a> PageMapperManager<'a> {
    pub fn new() -> Self {
        let pml4 = unsafe { request_page().unwrap().leak() };
        let page_mapper = unsafe { PageTable::<PageLvl4>::from_page(pml4) };
        Self {
            page_mapper,
            mappings: Vec::new(),
        }
    }

    /// Unsafe as this can't be dropped as we shim the alloc32page into allocpage
    pub unsafe fn new_32() -> Self {
        let pml4 = frame_alloc_exec(|a| a.request_32bit_reserved_page())
            .unwrap()
            .leak();
        let page_mapper = unsafe { PageTable::<PageLvl4>::from_page(pml4) };
        Self {
            page_mapper,
            mappings: Vec::new(),
        }
    }

    pub unsafe fn get_mapper_mut(&mut self) -> &mut PageTable<'a, PageLvl4> {
        &mut self.page_mapper
    }

    pub fn insert_mapping_at(
        &mut self,
        base: usize,
        mapping: Arc<PageMapping>,
        flags: MemoryMappingFlags,
    ) -> Option<()> {
        assert!(base & 0xFFF == 0);

        let end = base + mapping.size;

        for (r, ..) in &self.mappings {
            if base < r.end && r.start <= end {
                return None;
            }
        }

        let idx = self
            .mappings
            .binary_search_by(|(r, ..)| r.start.cmp(&base))
            .unwrap_err();

        self.mappings.insert(idx, ((base..end), mapping, flags));
        Some(())
    }

    pub fn insert_mapping_at_set(
        &mut self,
        base: usize,
        mapping: Arc<PageMapping>,
        flags: MemoryMappingFlags,
    ) -> Option<()> {
        assert!(base & 0xFFF == 0);

        let end = base + mapping.size;

        for (r, ..) in &self.mappings {
            if base < r.end && r.start <= end {
                return None;
            }
        }

        let idx = self
            .mappings
            .binary_search_by(|(r, ..)| r.start.cmp(&base))
            .unwrap_err();

        match &mapping.mapping {
            PageMappingType::MMAP { base_address } => {
                for (phys, virt) in
                    ((*base_address..).step_by(0x1000)).zip((base..end).step_by(0x1000))
                {
                    self.page_mapper
                        .map_memory(
                            Page::<Size4KB>::containing(virt as u64),
                            Page::<Size4KB>::new(phys as u64),
                            flags,
                        )
                        .unwrap()
                        .ignore();
                }
            }
            PageMappingType::LazyMapping { pages } => {
                for page in pages
                    .lock()
                    .iter()
                    .zip((base..end).step_by(0x1000))
                    .filter_map(|(a, i)| a.get().map(|p| (p, i)))
                {
                    self.page_mapper
                        .map_memory(Page::<Size4KB>::containing(page.1 as u64), page.0, flags)
                        .unwrap()
                        .ignore();
                }
            }
        }

        self.mappings.insert(idx, ((base..end), mapping, flags));

        // let m = self.mappings.remove(idx);

        Some(())
    }

    pub fn insert_mapping(
        &mut self,
        mapping: Arc<PageMapping>,
        flags: MemoryMappingFlags,
    ) -> usize {
        let idx = self
            .mappings
            .windows(2)
            .position(|window| {
                if let [left, right] = window {
                    // add some padding
                    left.0.end + 0x1000 + mapping.size <= right.0.start
                } else {
                    unreachable!()
                }
            })
            .expect("there should've been space somewhere");

        let base = self.mappings[idx].0.end + 0x1000;

        self.mappings
            .insert(idx + 1, ((base..base + mapping.size), mapping, flags));
        base
    }

    pub fn insert_mapping_set(
        &mut self,
        mapping: Arc<PageMapping>,
        flags: MemoryMappingFlags,
    ) -> usize {
        let idx = self
            .mappings
            .windows(2)
            .position(|window| {
                if let [left, right] = window {
                    // add some padding
                    left.0.end + 0x1000 + mapping.size <= right.0.start
                } else {
                    unreachable!()
                }
            })
            .expect("there should've been space somewhere");

        let base = self.mappings[idx].0.end + 0x1000;
        let end = base + mapping.size;

        match &mapping.mapping {
            PageMappingType::MMAP { base_address } => {
                for (phys, virt) in
                    ((*base_address..).step_by(0x1000)).zip((base..end).step_by(0x1000))
                {
                    self.page_mapper
                        .map_memory(
                            Page::<Size4KB>::containing(virt as u64),
                            Page::<Size4KB>::new(phys as u64),
                            flags,
                        )
                        .unwrap()
                        .ignore();
                }
            }
            PageMappingType::LazyMapping { pages } => {
                for page in pages
                    .lock()
                    .iter()
                    .zip((base..end).step_by(0x1000))
                    .filter_map(|(a, i)| a.get().map(|p| (p, i)))
                {
                    self.page_mapper
                        .map_memory(Page::<Size4KB>::containing(page.1 as u64), page.0, flags)
                        .unwrap()
                        .ignore();
                }
            }
        }

        self.mappings
            .insert(idx + 1, ((base..base + mapping.size), mapping, flags));
        base
    }

    pub fn page_fault_handler(&mut self, address: usize) -> Option<()> {
        if address > MemoryLoc::EndUserMem as usize {
            return None;
        }

        // find the mapping that address is in
        let idx = self.mappings.binary_search_by(|(r, ..)| {
            if r.start > address {
                Ordering::Greater
            } else if r.end <= address {
                Ordering::Less
            } else {
                Ordering::Equal
            }
        });
        let idx = idx.ok()?;

        let map = &mut self.mappings[idx];
        let offset = address - map.0.start;
        let phys = match &map.1.mapping {
            PageMappingType::MMAP { base_address } => {
                Page::containing((*base_address + offset) as u64)
            }
            PageMappingType::LazyMapping { pages } => {
                let idx = offset / 0x1000;
                let page = &mut pages.lock()[idx];
                page.0.unwrap_or_else(|| {
                    let apage = request_page().unwrap();
                    let p = *apage;
                    page.set(apage);
                    p
                })
            }
        };
        // Make the mapping
        match self
            .page_mapper
            .map_memory(Page::<Size4KB>::containing(address as u64), phys, map.2)
        {
            Ok(f) => f.flush(),
            Err(_) => (), // Already mapped ??
        }

        Some(())
    }

    pub unsafe fn free_mapping(&mut self, range: Range<usize>) -> Result<(), UnMapMemoryError> {
        let idx = self
            .mappings
            .binary_search_by(|el| el.0.clone().cmp(range.clone()))
            .map_err(|_| UnMapMemoryError::MemNotMapped(range.start as u64))?;

        let m = self.mappings.remove(idx);
        for page in m.0.step_by(0x1000) {
            match self
                .page_mapper
                .unmap_memory(Page::<Size4KB>::new(page as u64))
            {
                Ok(f) => f.flush(),
                Err(UnMapMemoryError::MemNotMapped(_)) => (),
                Err(e) => return Err(e),
            }

            // TODO: Send IPI to flush on other threads
        }
        Ok(())
    }
}

// If None, the page should be mapped to the ZERO page
pub struct MaybeAllocatedPage(Option<Page<Size4KB>>);

impl MaybeAllocatedPage {
    pub const fn new() -> Self {
        Self(None)
    }

    pub fn set(&mut self, page: AllocatedPage) -> Option<AllocatedPage> {
        let p = unsafe { page.leak() };
        let old = core::mem::replace(&mut self.0, Some(p))?;
        unsafe { Some(AllocatedPage::new(old)) }
    }

    pub fn get(&self) -> Option<Page<Size4KB>> {
        self.0
    }
}

impl From<AllocatedPage> for MaybeAllocatedPage {
    fn from(value: AllocatedPage) -> Self {
        Self(Some(unsafe { value.leak() }))
    }
}

impl Drop for MaybeAllocatedPage {
    fn drop(&mut self) {
        if let Some(p) = self.0 {
            unsafe { frame_alloc_exec(|a| a.free_page(p)) }
        }
    }
}
