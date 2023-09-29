use core::{cmp::Ordering, mem::MaybeUninit, ops::Range};

use alloc::{boxed::Box, vec::Vec};
use x86_64::{align_down, align_up};

use super::{
    page_allocator::{frame_alloc_exec, request_page, AllocatedPage},
    page_table_manager::{Flusher, Mapper, Page, PageLvl4, PageTable, Size4KB},
    MemoryLoc,
};

pub struct PageMapperManager<'a> {
    _pml4: AllocatedPage,
    page_mapper: PageTable<'a, PageLvl4>,
    // start offset, end offset, mapping
    // this should always be ordered
    mappings: Vec<(Range<usize>, Box<[MaybeAllocatedPage]>)>,
}

impl<'a> PageMapperManager<'a> {
    pub fn new() -> Self {
        let pml4 = request_page().unwrap();
        let page_mapper = unsafe { PageTable::<PageLvl4>::from_page(*pml4) };
        Self {
            _pml4: pml4,
            page_mapper,
            mappings: Vec::new(),
        }
    }

    pub unsafe fn get_mapper_mut(&mut self) -> &mut PageTable<'a, PageLvl4> {
        &mut self.page_mapper
    }

    pub fn create_lazy_mapping(&mut self, start: usize, length: usize) -> usize {
        let mut start = align_down(start as u64, 0x1000) as usize;
        let length = align_up(length as u64, 0x1000) as usize;
        let end;

        // get next available address
        if start == 0 {
            for maps in self.mappings.windows(2) {
                // if there is enough space use it
                if maps[0].0.end + 0x1000 + length <= maps[1].0.start {
                    start = maps[0].0.end + 0x1000;
                    break;
                }
            }
            if start == 0 {
                start = self.mappings.last().unwrap().0.end;
            }
            end = start + length;
        } else {
            end = start + length;
            for (r, _) in &self.mappings {
                if start <= r.end && r.start <= end {
                    panic!("mapping already exists in the range")
                }
            }
        }

        if end > MemoryLoc::EndUserMem as usize {
            panic!("cannot create kmapping")
        }

        let npages = length / 0x1000;
        let b = unsafe {
            let mut b = Box::new_uninit_slice(npages);
            b.fill_with(|| MaybeUninit::new(MaybeAllocatedPage::new()));
            b.assume_init()
        };

        let idx = self
            .mappings
            .binary_search_by(|(r, _)| r.start.cmp(&start))
            .unwrap_err();

        self.mappings.insert(idx, (start..end, b));
        start
    }

    pub fn create_mapping_with_alloc(
        &mut self,
        start: usize,
        length: usize,
        pages: Box<[MaybeAllocatedPage]>,
    ) {
        assert_eq!(start & 0xFFF, 0);
        let length = align_up(length as u64, 0x1000) as usize;
        let end = start + length;
        if end > MemoryLoc::EndUserMem as usize {
            panic!("cannot create kmapping")
        }

        for (r, _) in &self.mappings {
            if start < r.end && r.start <= end {
                panic!(
                    "mapping already exists in the range {:#x}:{:#x} {:#x}:{:#x}",
                    start, r.end, r.start, end
                )
            }
        }

        // map all allocated pages
        for (page, virt_addr) in pages
            .iter()
            .zip((start..).step_by(0x1000))
            .filter_map(|(p, a)| Some((p.get()?, a)))
        {
            self.page_mapper
                .map_memory(Page::new(virt_addr as u64), page)
                // .map_err(|_| LoadElfError::InternalError)?
                .unwrap()
                .ignore();
        }

        let idx = self
            .mappings
            .binary_search_by(|(r, _)| r.start.cmp(&start))
            .unwrap_err();

        self.mappings.insert(idx, (start..end, pages));
    }

    pub fn page_fault_handler(&mut self, address: usize) {
        if address > MemoryLoc::EndUserMem as usize {
            panic!("kmap page fault")
        }

        // find the mapping that address is in
        let idx = self
            .mappings
            .binary_search_by(|(r, _)| {
                if r.start > address {
                    Ordering::Greater
                } else if r.end <= address {
                    Ordering::Less
                } else {
                    Ordering::Equal
                }
            })
            .unwrap();

        let map = &mut self.mappings[idx];
        let idx = ((address & !0xfff) - map.0.start) / 0x1000;
        let page = &mut map.1[idx];
        match page.0 {
            Some(p) => {
                // page was mapped but not flushed
                Flusher::new(p.get_address()).flush();
            }
            None => {
                let apage = request_page().unwrap();
                self.page_mapper
                    .map_memory(Page::containing(address as u64), *apage)
                    .unwrap()
                    .flush();
                page.set(apage);
            }
        }
    }

    pub unsafe fn free_mapping(&mut self, start: usize, length: usize) {
        assert_eq!(start & 0xFFF, 0);
        let length = align_up(length as u64, 0x1000) as usize;
        let end = start + length;

        let idx = self
            .mappings
            .binary_search_by(|(r, _)| r.start.cmp(&start))
            .unwrap();
        let m = self.mappings.remove(idx);
        assert_eq!(m.0.len(), length);
        for page in m.1.iter().zip((start..end).step_by(0x1000)) {
            if let Some(_) = page.0 .0 {
                self.page_mapper
                    .unmap_memory(Page::<Size4KB>::new(page.1 as u64))
                    .unwrap()
                    .flush();
                // TODO: Send IPI to flush on other threads
            }
        }
    }
}

// If None, the page should be mapped to the ZERO page
pub struct MaybeAllocatedPage(Option<Page<Size4KB>>);

impl MaybeAllocatedPage {
    pub fn new() -> Self {
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
