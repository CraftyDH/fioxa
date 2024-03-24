pub mod map;
pub mod walk;

use core::{marker::PhantomData, num::NonZeroU64};

use thiserror::Error;
use x86_64::instructions::interrupts::without_interrupts;

use crate::{cpu_localstorage::CPULocalStorageRW, paging::page_allocator::free_page_early};

use super::{
    page_allocator::request_page, page_directory::PageDirectoryEntry, phys_addr_for_virt,
    virt_addr_for_phys, MemoryMappingFlags,
};

#[derive(Error, Debug)]
pub enum MapMemoryError {
    #[error("cannot map {from:X} to {to:X} because {current:X} is mapped")]
    MemAlreadyMapped { from: u64, to: u64, current: u64 },
}

#[derive(Error, Debug)]
pub enum UnMapMemoryError {
    #[error("cannot unmap {0} because it is not mapped")]
    MemNotMapped(u64),
    #[error("cannot unmap {0} because the page tables don't exist")]
    PathNotFound(u64),
}

#[derive(Debug)]
pub struct Size4KB;
#[derive(Debug)]

pub struct Size2MB;
#[derive(Debug)]
pub struct Size1GB;

pub trait PageSize: Sized {
    const LARGE_PAGE: bool = true;
    /// The size of the page in bytes
    const PAGE_SIZE: u64;
}

impl PageSize for Size4KB {
    const LARGE_PAGE: bool = false;
    const PAGE_SIZE: u64 = 0x1000;
}
impl PageSize for Size2MB {
    const PAGE_SIZE: u64 = 0x200000;
}
impl PageSize for Size1GB {
    const PAGE_SIZE: u64 = 0x40000000;
}

#[derive(Debug)]
pub struct Page<S: PageSize> {
    address: NonZeroU64,
    _size: core::marker::PhantomData<S>,
}

impl<S: PageSize> Page<S> {
    pub fn get_address(&self) -> u64 {
        self.address.into()
    }

    pub fn new(address: u64) -> Self {
        assert!(
            address & (S::PAGE_SIZE - 1) == 0,
            "Address must be a multiple of page size"
        );

        let lvl4 = (address >> (12 + 9 + 9 + 9)) & 0x1ff;
        let sign = address >> (12 + 9 + 9 + 9 + 9);

        assert!(
            (sign == 0 && lvl4 <= 255) || (sign == 0xFFFF && lvl4 > 255),
            "Sign extension is not valid"
        );

        Page {
            address: address.try_into().expect("page address should not be zero"),
            _size: core::marker::PhantomData,
        }
    }

    pub fn containing(address: u64) -> Self {
        Self::new(address & !(S::PAGE_SIZE - 1))
    }
}

impl<S: PageSize> Copy for Page<S> {}

impl<S: PageSize> Clone for Page<S> {
    fn clone(&self) -> Self {
        *self
    }
}

pub trait Mapper<S: PageSize> {
    fn map_memory(
        &mut self,
        from: Page<S>,
        to: Page<S>,
        flags: MemoryMappingFlags,
    ) -> Result<Flusher, MapMemoryError>;
    #[inline]
    fn identity_map_memory(
        &mut self,
        page: Page<S>,
        flags: MemoryMappingFlags,
    ) -> Result<Flusher, MapMemoryError> {
        self.map_memory(page, page, flags)
    }
    fn unmap_memory(&mut self, page: Page<S>) -> Result<Flusher, UnMapMemoryError>;
    fn get_phys_addr(&mut self, page: Page<S>) -> Option<u64>;
}

pub struct PageLvl1;
pub struct PageLvl2;
pub struct PageLvl3;
pub struct PageLvl4;

pub trait PageLevel {
    const INDEXER: usize;

    /// Calulates the index of the page table entry at the given level
    fn calc_idx(address: u64) -> usize {
        (address as usize >> Self::INDEXER) & 0x1ff
    }
}

impl PageLevel for PageLvl4 {
    const INDEXER: usize = 12 + 9 + 9 + 9;
}

impl PageLevel for PageLvl3 {
    const INDEXER: usize = 12 + 9 + 9;
}

impl PageLevel for PageLvl2 {
    const INDEXER: usize = 12 + 9;
}

impl PageLevel for PageLvl1 {
    const INDEXER: usize = 12;
}
pub trait NextLevel {
    type Next: PageLevel;
}

impl NextLevel for PageLvl4 {
    type Next = PageLvl3;
}

impl NextLevel for PageLvl3 {
    type Next = PageLvl2;
}

impl NextLevel for PageLvl2 {
    type Next = PageLvl1;
}
pub trait LvlSize {
    type Size: PageSize;
}

impl LvlSize for PageLvl3 {
    type Size = Size1GB;
}

impl LvlSize for PageLvl2 {
    type Size = Size2MB;
}

impl LvlSize for PageLvl1 {
    type Size = Size4KB;
}

#[repr(C, align(0x1000))]
pub struct PhysPageTable {
    entries: [PageDirectoryEntry; 512],
}

impl PhysPageTable {
    fn get_or_create_table(&mut self, idx: usize) -> &mut PhysPageTable {
        let entry = &mut self.entries[idx];
        if entry.larger_pages() {
            panic!("Page Lvl4 cannot contain huge pages")
        }
        let addr;
        if entry.present() {
            addr = virt_addr_for_phys(entry.get_address()) as *mut PhysPageTable;
        } else {
            let new_page = unsafe { request_page().unwrap().leak() };
            addr = virt_addr_for_phys(new_page.get_address()) as *mut PhysPageTable;

            entry.set_address(new_page.get_address());
            entry.set_present(true);
            entry.set_read_write(true);
            entry.set_user_super(true);
        }

        unsafe { &mut *addr }
    }

    unsafe fn free_table(&mut self, idx: usize) {
        let entry = &mut self.entries[idx];
        assert!(entry.present());
        let phys = Page::<Size4KB>::new(entry.get_address());
        free_page_early(phys);
        entry.set_present(false);
        entry.set_address(0);
    }

    fn set_table(&mut self, idx: usize, table: &mut PhysPageTable) {
        let entry = &mut self.entries[idx];
        if entry.present() {
            println!("WARN: setting table over existing entry");
        }
        entry.set_address(table.entries.as_ptr() as u64);
        entry.set_present(true);
        entry.set_read_write(true);
        entry.set_user_super(true);
    }

    fn get_table(&self, idx: usize) -> Option<&mut PhysPageTable> {
        let entry = &self.entries[idx];
        if entry.larger_pages() {
            panic!("Page Lvl4 cannot contain huge pages")
        }
        if entry.present() {
            Some(unsafe { &mut *(virt_addr_for_phys(entry.get_address()) as *mut PhysPageTable) })
        } else {
            None
        }
    }

    fn is_empty(&self) -> bool {
        self.entries.iter().all(|e| !e.present())
    }
}

pub struct PageTable<'t, L: PageLevel> {
    table: &'t mut PhysPageTable,
    level: core::marker::PhantomData<L>,
}

impl<L: PageLevel> PageTable<'_, L> {
    pub unsafe fn from_page(page: Page<Size4KB>) -> Self {
        let table = virt_addr_for_phys(page.get_address()) as *mut PhysPageTable;

        PageTable {
            table: unsafe { &mut *table },
            level: core::marker::PhantomData,
        }
    }

    pub fn into_page(&self) -> Page<Size4KB> {
        Page::new(phys_addr_for_virt(self.table as *const _ as u64))
    }
}

impl<S: PageLevel + NextLevel> PageTable<'_, S> {
    pub fn get_next_table<P: PageSize>(&mut self, address: Page<P>) -> PageTable<'_, S::Next> {
        let table = self
            .table
            .get_or_create_table(S::calc_idx(address.get_address()));
        PageTable {
            table,
            level: core::marker::PhantomData,
        }
    }

    pub fn try_get_next_table<P: PageSize>(
        &self,
        address: Page<P>,
    ) -> Option<PageTable<'_, S::Next>> {
        let table = self.table.get_table(S::calc_idx(address.get_address()))?;
        Some(PageTable {
            table,
            level: core::marker::PhantomData,
        })
    }

    pub unsafe fn set_next_table(&mut self, address: u64, table: &mut PageTable<'_, S::Next>) {
        self.table
            .set_table(PageLvl4::calc_idx(address), table.table);
    }

    pub fn unmap_memory_walk_inner<P: PageSize>(
        &mut self,
        page: Page<P>,
    ) -> Result<Flusher, UnMapMemoryError>
    where
        for<'a> PageTable<'a, S::Next>: Mapper<P>,
    {
        self.get_next_table(page).unmap_memory(page).and_then(|r| {
            let table = self
                .table
                .get_table(S::calc_idx(page.get_address()))
                .ok_or(UnMapMemoryError::PathNotFound(page.get_address()))?;
            if table.is_empty() {
                unsafe { self.table.free_table(S::calc_idx(page.get_address())) }
            }
            Ok(r)
        })
    }
}

impl<S: PageLevel + LvlSize> PageTable<'_, S> {
    fn get_entry_mut(&mut self, page: Page<S::Size>) -> &mut PageDirectoryEntry {
        &mut self.table.entries[S::calc_idx(page.get_address())]
    }

    fn get_entry(&self, page: Page<S::Size>) -> &PageDirectoryEntry {
        &self.table.entries[S::calc_idx(page.get_address())]
    }

    fn map_memory_inner(
        &mut self,
        from: Page<S::Size>,
        to: Page<S::Size>,
        flags: MemoryMappingFlags,
    ) -> Result<Flusher, MapMemoryError> {
        let entry = self.get_entry_mut(from);

        // TODO: Stop overriding exiting mappings
        if entry.present() {
            // TODO: When the kernel is less buggy by double mapping, assert this whenever it is present
            // Make sure we aren't creating bugs by checking that it only overrides with the same addr
            // if entry.get_address() != to.address {
            return Err(MapMemoryError::MemAlreadyMapped {
                from: from.get_address(),
                to: to.get_address(),
                current: entry.get_address(),
            });
            // }
        }

        entry.set_present(true);
        entry.set_larger_pages(S::Size::LARGE_PAGE);
        entry.set_address(to.address.into());
        entry.set_read_write(flags.contains(MemoryMappingFlags::WRITEABLE));
        entry.set_user_super(flags.contains(MemoryMappingFlags::USERSPACE));

        Ok(Flusher(from.address.into()))
    }

    fn unmap_memory_inner(&mut self, page: Page<S::Size>) -> Result<Flusher, UnMapMemoryError> {
        let entry = self.get_entry_mut(page);

        if !entry.present() {
            return Err(UnMapMemoryError::MemNotMapped(page.get_address()));
        }

        entry.set_present(false);
        entry.set_address(0);

        Ok(Flusher(page.address.into()))
    }

    fn get_phys_addr_inner(&self, page: Page<S::Size>) -> Option<u64> {
        let entry = self.get_entry(page);

        if !entry.present() {
            return None;
        }

        if entry.larger_pages() {
            // TODO: Better error
            todo!()
        } else {
            Some(entry.get_address())
        }
    }
}

pub unsafe fn ensure_ident_map_curr_process<S: PageSize>(page: Page<S>, flags: MemoryMappingFlags)
where
    for<'a> PageTable<'a, PageLvl4>: Mapper<S>,
{
    without_interrupts(|| {
        let proc = CPULocalStorageRW::get_current_task().process();
        let mut mem = proc.memory.lock();
        let mapper = mem.page_mapper.get_mapper_mut();
        match mapper.identity_map_memory(page, flags) {
            Ok(f) => f.flush(),
            Err(MapMemoryError::MemAlreadyMapped {
                from: _,
                to,
                current,
            }) if to == current => (),
            Err(e) => panic!("cannot ident map because {e:?}"),
        }
    })
}

#[must_use = "TLB must be flushed or can be ignored"]
pub struct Flusher(u64);

impl Flusher {
    pub fn new(addr: u64) -> Self {
        Self(addr)
    }

    pub fn flush(self) {
        unsafe {
            core::arch::asm!("invlpg [{}]", in(reg) self.0, options(nostack, preserves_flags))
        }
    }

    pub fn ignore(self) {}
}

pub fn get_chunked_page_range(
    mut start: u64,
    mut end: u64,
) -> (
    PageRange<Size4KB>,
    PageRange<Size2MB>,
    PageRange<Size1GB>,
    PageRange<Size2MB>,
    PageRange<Size4KB>,
) {
    assert!(start & 0xFFF == 0);
    assert!(end & 0xFFF == 0);

    // Normalize 4kb chunks
    let s4kb = if start & 0x1f_ffff > 0 {
        let new_start = core::cmp::min((start & !0x1f_ffff) + 0x200000, end);
        let tmp = PageRange::new(
            start,
            (new_start - start) as usize / Size4KB::PAGE_SIZE as usize,
        );
        start = new_start;
        tmp
    } else {
        PageRange::empty()
    };

    let e4kb = if end & 0x1f_ffff > 0 && start != end {
        let new_end = core::cmp::max(end & !0x1f_ffff, start);
        let tmp = PageRange::new(
            new_end,
            (end - new_end) as usize / Size4KB::PAGE_SIZE as usize,
        );
        end = new_end;
        tmp
    } else {
        PageRange::empty()
    };

    // Normalize 2mb chunks
    let s2mb = if start & 0x3fffffff > 0 && start != end {
        let new_start = core::cmp::min((start & !0x3fffffff) + 0x40000000, end);
        let tmp = PageRange::new(
            start,
            (new_start - start) as usize / Size2MB::PAGE_SIZE as usize,
        );
        start = new_start;
        tmp
    } else {
        PageRange::empty()
    };

    let e2mb = if end & 0x3fffffff > 0 && start != end {
        let new_end = core::cmp::max(end & !0x3fffffff, start);

        let tmp = PageRange::new(
            new_end,
            (end - new_end) as usize / Size2MB::PAGE_SIZE as usize,
        );
        end = new_end;
        tmp
    } else {
        PageRange::empty()
    };

    let gb = if start < end && start != end {
        PageRange::new(start, ((end - start) / Size1GB::PAGE_SIZE) as usize)
    } else {
        PageRange::empty()
    };

    (s4kb, s2mb, gb, e2mb, e4kb)
}

#[derive(Debug)]
pub struct PageRange<S: PageSize> {
    base: u64,
    count: usize,
    _size: PhantomData<S>,
}

impl<S: PageSize> PageRange<S> {
    pub fn new(base: u64, count: usize) -> Self {
        Self {
            base,
            count,
            _size: Default::default(),
        }
    }

    pub fn empty() -> Self {
        Self::new(0, 0)
    }
}

impl<S: PageSize> Iterator for PageRange<S> {
    type Item = Page<S>;

    fn next(&mut self) -> Option<Self::Item> {
        if self.count > 0 {
            let res = Page::new(self.base);
            self.count -= 1;
            self.base += S::PAGE_SIZE;
            Some(res)
        } else {
            None
        }
    }

    fn size_hint(&self) -> (usize, Option<usize>) {
        (self.count, Some(self.count))
    }
}

impl<S: PageSize> ExactSizeIterator for PageRange<S> {
    fn len(&self) -> usize {
        self.count
    }
}
