use core::marker::PhantomData;

use conquer_once::spin::Lazy;
use thiserror::Error;

use crate::{mutex::Spinlock, paging::page::Page};

use super::{
    KERNEL_DATA_MAP, KERNEL_HEAP_MAP, MemoryLoc, MemoryMappingFlags, OFFSET_MAP, PER_CPU_MAP,
    PageAllocator,
    page::{PageSize, Size1GB, Size2MB, Size4KB},
    page_directory::PageDirectoryEntry,
    virt_addr_offset_mut,
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

#[repr(C, align(0x1000))]
pub struct PhysPageTable {
    entries: [PageDirectoryEntry; 512],
}

impl PhysPageTable {
    pub fn all_entries_empty(&self) -> bool {
        !self.entries.iter().any(|e| e.present())
    }
}

pub struct PageTable<L: TableLevel> {
    // Physical address of the table
    table: *mut PhysPageTable,
    _level: PhantomData<L>,
}

unsafe impl<L: TableLevel> Send for PageTable<L> {}

pub trait TableLevel {
    const LEVEL: usize;

    const INDEXER: usize = Self::LEVEL * 9 + 3;

    fn calculate_index(address: usize) -> usize {
        (address >> Self::INDEXER) & 0x1ff
    }

    fn calculate_index_page<P: PageSize>(page: Page<P>) -> usize {
        Self::calculate_index(page.get_address() as usize)
    }
}

trait TableLevelNext {
    type Next: TableLevel;
}

pub trait TableLevelMap {
    type Size: PageSize;
    const LARGER_PAGES: bool = true;
}

pub struct TableLevel1;
pub struct TableLevel2;
pub struct TableLevel3;
pub struct TableLevel4;

impl TableLevel for TableLevel1 {
    const LEVEL: usize = 1;
}

impl TableLevel for TableLevel2 {
    const LEVEL: usize = 2;
}

impl TableLevel for TableLevel3 {
    const LEVEL: usize = 3;
}

impl TableLevel for TableLevel4 {
    const LEVEL: usize = 4;
}

impl TableLevelNext for TableLevel4 {
    type Next = TableLevel3;
}

impl TableLevelNext for TableLevel3 {
    type Next = TableLevel2;
}

impl TableLevelNext for TableLevel2 {
    type Next = TableLevel1;
}

impl TableLevelMap for TableLevel1 {
    type Size = Size4KB;
    const LARGER_PAGES: bool = false;
}

impl TableLevelMap for TableLevel2 {
    type Size = Size2MB;
}

impl TableLevelMap for TableLevel3 {
    type Size = Size1GB;
}

pub trait Mapper<P: PageSize> {
    fn map(
        &mut self,
        alloc: &impl PageAllocator,
        virtual_page: Page<P>,
        physical_page: Page<P>,
        flags: MemoryMappingFlags,
    ) -> Result<Flusher, MapMemoryError>;

    fn unmap(
        &mut self,
        alloc: &impl PageAllocator,
        page: Page<P>,
    ) -> Result<Flusher, UnMapMemoryError>;

    fn address_of(&self, page: Page<P>) -> Option<Page<P>>;

    fn identity_map(
        &mut self,
        alloc: &impl PageAllocator,
        page: Page<P>,
        flags: MemoryMappingFlags,
    ) -> Result<Flusher, MapMemoryError> {
        self.map(alloc, page, page, flags)
    }
}

macro_rules! gen_base_tables {
    ($($table:ty),+) => {
        $(
            impl Mapper<<$table as TableLevelMap>::Size> for PageTable<$table> {
                fn map(&mut self,
                    alloc: &impl PageAllocator,
                    virtual_page: Page<<$table as TableLevelMap>::Size>,
                    physical_page: Page<<$table as TableLevelMap>::Size>,
                    flags: MemoryMappingFlags,
                ) -> Result<Flusher, MapMemoryError> {
                    self.map_inner(alloc, virtual_page, physical_page, flags)
                }

                fn unmap(
                    &mut self,
                    alloc: &impl PageAllocator,
                    page: Page<<$table as TableLevelMap>::Size>,
                ) -> Result<Flusher, UnMapMemoryError>{
                    self.unmap_inner(alloc, page)
                }

                fn address_of(&self, page: Page<<$table as TableLevelMap>::Size>) -> Option<Page<<$table as TableLevelMap>::Size>> {
                    self.address_of_inner(page)
                }
            }
        )*
    };
}

gen_base_tables!(TableLevel1, TableLevel2, TableLevel3);

impl PageTable<TableLevel4> {
    pub fn new_with_global(alloc: &impl PageAllocator) -> Self {
        let this = Self::new(alloc);
        let table = this.table();

        let mut set = |addr, t: &Lazy<Spinlock<PageTable<TableLevel3>>>| {
            let e = &mut table.entries[TableLevel4::calculate_index(addr)];
            e.set_present(true);
            e.set_read_write(true);
            e.set_user_super(false);
            e.set_address(t.lock().get_physical_address() as u64);
        };
        set(MemoryLoc::PhysMapOffset as usize, &OFFSET_MAP);
        set(MemoryLoc::KernelStart as usize, &KERNEL_DATA_MAP);
        set(MemoryLoc::KernelHeap as usize, &KERNEL_HEAP_MAP);
        set(MemoryLoc::PerCpuMem as usize, &PER_CPU_MAP);
        this
    }

    pub fn get_phys_addr_from_vaddr(&self, address: u64) -> Option<u64> {
        Some(
            self.address_of(Page::<Size4KB>::containing(address))?
                .get_address()
                + address % 0x1000,
        )
    }
}

impl<L: TableLevel> PageTable<L> {
    pub fn new(alloc: &impl PageAllocator) -> Self {
        unsafe {
            let page = alloc.allocate_page().unwrap().get_address();
            Self::from_raw(page as *mut PhysPageTable)
        }
    }

    pub unsafe fn from_raw(table: *mut PhysPageTable) -> Self {
        Self {
            table,
            _level: PhantomData,
        }
    }

    pub fn get_physical_address(&self) -> usize {
        self.table.addr()
    }

    fn table(&self) -> &mut PhysPageTable {
        unsafe { &mut *virt_addr_offset_mut(self.table) }
    }
}

impl<L: TableLevel + TableLevelMap> PageTable<L> {
    fn map_inner(
        &mut self,
        _: &impl PageAllocator,
        virtual_page: Page<L::Size>,
        physical_page: Page<L::Size>,
        flags: MemoryMappingFlags,
    ) -> Result<Flusher, MapMemoryError> {
        let index = L::calculate_index_page(virtual_page);
        let table = self.table();

        let e = &mut table.entries[index];

        if e.present() {
            return Err(MapMemoryError::MemAlreadyMapped {
                from: virtual_page.get_address(),
                to: physical_page.get_address(),
                current: e.get_address(),
            });
        }

        e.set_present(true);
        e.set_larger_pages(L::LARGER_PAGES);
        e.set_read_write(flags.contains(MemoryMappingFlags::WRITEABLE));
        e.set_user_super(flags.contains(MemoryMappingFlags::USERSPACE));
        e.set_address(physical_page.get_address());
        Ok(Flusher(virtual_page.get_address()))
    }

    fn unmap_inner(
        &mut self,
        _: &impl PageAllocator,
        page: Page<L::Size>,
    ) -> Result<Flusher, UnMapMemoryError> {
        let index = L::calculate_index_page(page);
        let table = self.table();

        let e = &mut table.entries[index];

        if !e.present() {
            // allow unmap of nothing
            return Err(UnMapMemoryError::MemNotMapped(page.get_address()));
        }
        assert_eq!(e.larger_pages(), L::LARGER_PAGES);

        e.set_present(false);
        e.set_address(0);
        Ok(Flusher(page.get_address()))
    }

    fn address_of_inner(&self, page: Page<L::Size>) -> Option<Page<L::Size>> {
        let index = L::calculate_index_page(page);
        let table = self.table();

        let e = &table.entries[index];

        if e.present() && L::LARGER_PAGES == e.larger_pages() {
            Some(Page::new(e.get_address()))
        } else {
            None
        }
    }
}

impl<L: TableLevel + TableLevelNext, P: PageSize> Mapper<P> for PageTable<L>
where
    PageTable<L::Next>: Mapper<P>,
{
    fn map(
        &mut self,
        alloc: &impl PageAllocator,
        virtual_page: Page<P>,
        physical_page: Page<P>,
        flags: MemoryMappingFlags,
    ) -> Result<Flusher, MapMemoryError> {
        let index = L::calculate_index_page(virtual_page);
        let table = self.table();

        let e = &mut table.entries[index];

        let mut next: PageTable<L::Next> = if e.present() {
            assert!(!e.larger_pages(), "map over larger pages");
            unsafe { PageTable::from_raw(e.get_address() as *mut PhysPageTable) }
        } else {
            let table = PageTable::new(alloc);

            e.set_present(true);
            e.set_larger_pages(false);
            e.set_read_write(true);
            e.set_user_super(true);
            e.set_address(table.get_physical_address() as u64);

            table
        };

        next.map(alloc, virtual_page, physical_page, flags)
    }

    fn unmap(
        &mut self,
        alloc: &impl PageAllocator,
        page: Page<P>,
    ) -> Result<Flusher, UnMapMemoryError> {
        let index = L::calculate_index_page(page);
        let table = self.table();

        let e = &mut table.entries[index];

        if !e.present() {
            return Err(UnMapMemoryError::MemNotMapped(page.get_address()));
        }
        assert!(!e.larger_pages(), "unmap over larger pages");

        let mut next: PageTable<L::Next> =
            unsafe { PageTable::from_raw(e.get_address() as *mut PhysPageTable) };
        let r = next.unmap(alloc, page);

        // try cleaning up memory
        if next.table().all_entries_empty() {
            let page = Page::<Size4KB>::new(next.table as u64);
            drop(next);
            e.set_present(false);
            e.set_address(0);
            unsafe { alloc.free_page(page) };
        }
        r
    }

    fn address_of(&self, page: Page<P>) -> Option<Page<P>> {
        let index = L::calculate_index_page(page);
        let table = self.table();

        let e = &table.entries[index];

        if e.present() {
            assert!(!e.larger_pages());
            let next: PageTable<L::Next> =
                unsafe { PageTable::from_raw(e.get_address() as *mut PhysPageTable) };
            next.address_of(page)
        } else {
            None
        }
    }
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
