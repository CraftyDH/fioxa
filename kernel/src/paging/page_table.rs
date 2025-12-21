use core::{convert::Infallible, marker::PhantomData, mem::ManuallyDrop};

use kernel_sys::types::VMMapFlags;

use crate::paging::{
    AllocatedPage, GlobalPageAllocator, KERNEL_DATA_MAP, KERNEL_HEAP_MAP, KERNEL_STACKS_MAP,
    MemoryLoc, OFFSET_MAP, PER_CPU_MAP, page::Page, page_allocator::global_allocator,
    virt_addr_offset,
};

use super::{
    PageAllocator,
    page::{PageSize, Size1GB, Size2MB, Size4KB},
    page_directory::PageDirectoryEntry,
    virt_addr_offset_mut,
};

#[repr(transparent)]
pub struct DirectoryEntry<L> {
    entry: PageDirectoryEntry,
    _level: PhantomData<L>,
}

#[repr(C, align(0x1000))]
pub struct PageTable<L> {
    entries: [DirectoryEntry<L>; 512],
}

impl<L: TableLevel> PageTable<L> {
    pub fn all_entries_empty(&self) -> bool {
        self.entries.iter().all(|e| !e.present())
    }
}

pub struct PageTableOwned<L: TableLevel>
where
    PageTable<L>: TableOperations,
{
    table: *mut PageTable<L>,
}

impl<L: TableLevel> PageTableOwned<L>
where
    PageTable<L>: TableOperations,
{
    pub fn new(alloc: impl PageAllocator) -> Option<Self> {
        alloc.allocate_page().map(|page| Self {
            table: page.get_address() as *mut _,
        })
    }

    pub fn raw(&self) -> *mut PageTable<L> {
        self.table
    }

    pub fn leak(self) -> PageTableStatic<L> {
        PageTableStatic {
            table: ManuallyDrop::new(self).table,
        }
    }
}

impl<L: TableLevel> AsRef<PageTable<L>> for PageTableOwned<L>
where
    PageTable<L>: TableOperations,
{
    fn as_ref(&self) -> &PageTable<L> {
        unsafe { &*virt_addr_offset(self.table) }
    }
}

impl<L: TableLevel> AsMut<PageTable<L>> for PageTableOwned<L>
where
    PageTable<L>: TableOperations,
{
    fn as_mut(&mut self) -> &mut PageTable<L> {
        unsafe { &mut *virt_addr_offset_mut(self.table) }
    }
}

impl<L: TableLevel> Drop for PageTableOwned<L>
where
    PageTable<L>: TableOperations,
{
    fn drop(&mut self) {
        self.as_mut().clear();
        let page = Page::<Size4KB>::new(self.table as u64);
        unsafe { global_allocator().free_page(page) };
    }
}

pub struct PageTableStatic<L: TableLevel> {
    table: *mut PageTable<L>,
}

impl<L: TableLevel> PageTableStatic<L> {
    pub fn raw(&self) -> *mut PageTable<L> {
        self.table
    }
}

impl<L: TableLevel> AsRef<PageTable<L>> for PageTableStatic<L>
where
    PageTable<L>: TableOperations,
{
    fn as_ref(&self) -> &PageTable<L> {
        unsafe { &*virt_addr_offset(self.table) }
    }
}

impl<L: TableLevel> AsMut<PageTable<L>> for PageTableStatic<L>
where
    PageTable<L>: TableOperations,
{
    fn as_mut(&mut self) -> &mut PageTable<L> {
        unsafe { &mut *virt_addr_offset_mut(self.table) }
    }
}

impl<L: TableLevel> Clone for PageTableStatic<L>
where
    PageTable<L>: TableOperations,
{
    fn clone(&self) -> Self {
        Self { table: self.table }
    }
}

unsafe impl<L: TableLevel> Send for PageTableOwned<L> where PageTable<L>: TableOperations {}
unsafe impl<L: TableLevel> Send for PageTableStatic<L> where PageTable<L>: TableOperations {}

pub trait TableLevel {
    const LEVEL: usize;
    const INDEXER: usize = Self::LEVEL * 9 + 3;
    const LARGER_PAGES: bool = true;

    fn calculate_index(address: usize) -> usize {
        (address >> Self::INDEXER) & 0x1ff
    }

    type Next;
    type Map;
}

pub struct TableLevel1;
pub struct TableLevel2;
pub struct TableLevel3;
pub struct TableLevel4;

impl TableLevel for TableLevel1 {
    const LEVEL: usize = 1;
    const LARGER_PAGES: bool = false;

    type Next = Infallible;
    type Map = Size4KB;
}

impl TableLevel for TableLevel2 {
    const LEVEL: usize = 2;

    type Next = TableLevel1;
    type Map = Size2MB;
}

impl TableLevel for TableLevel3 {
    const LEVEL: usize = 3;

    type Next = TableLevel2;
    type Map = Size1GB;
}

impl TableLevel for TableLevel4 {
    const LEVEL: usize = 4;

    type Next = TableLevel3;
    type Map = Infallible;
}

impl<L: TableLevel> PageTable<L> {
    pub fn get(&self, vaddr: usize) -> &DirectoryEntry<L> {
        &self.entries[L::calculate_index(vaddr)]
    }

    pub fn get_mut(&mut self, vaddr: usize) -> &mut DirectoryEntry<L> {
        &mut self.entries[L::calculate_index(vaddr)]
    }
}

impl<L: TableLevel> DirectoryEntry<L> {
    pub fn present(&self) -> bool {
        self.entry.present()
    }

    pub fn fx_owned(&self) -> bool {
        self.present() && self.entry.fx_owned()
    }

    pub fn flags(&self) -> VMMapFlags {
        let mut flags = VMMapFlags::empty();
        if self.entry.read_write() {
            flags |= VMMapFlags::WRITEABLE;
        }
        if self.entry.user_super() {
            flags |= VMMapFlags::USERSPACE;
        }
        flags
    }

    pub fn set_flags(&mut self, flags: VMMapFlags) {
        self.entry
            .set_read_write(flags.contains(VMMapFlags::WRITEABLE));
        self.entry
            .set_user_super(flags.contains(VMMapFlags::USERSPACE));
    }

    pub fn upgrade_flags(&mut self, flags: VMMapFlags) {
        if flags.contains(VMMapFlags::WRITEABLE) {
            self.entry.set_read_write(true);
        }
        if flags.contains(VMMapFlags::USERSPACE) {
            self.entry.set_user_super(true);
        }
    }
}

impl<L: TableLevel> DirectoryEntry<L>
where
    L::Next: TableLevel,
    PageTable<L::Next>: TableOperations,
{
    pub fn is_table(&self) -> bool {
        self.present() && !self.entry.larger_pages()
    }

    pub unsafe fn unchecked_table(&self) -> &PageTable<L::Next> {
        unsafe { &*virt_addr_offset(self.entry.get_address() as *const _) }
    }

    pub unsafe fn unchecked_table_mut(&mut self) -> &mut PageTable<L::Next> {
        unsafe { &mut *virt_addr_offset_mut(self.entry.get_address() as *mut _) }
    }

    pub unsafe fn unchecked_take_table(&mut self) -> MaybeOwnedTable<L> {
        let table = self.entry.get_address() as *mut _;
        let owned = self.entry.fx_owned();
        self.entry = PageDirectoryEntry::new();
        if owned {
            MaybeOwned::Owned(PageTableOwned { table })
        } else {
            MaybeOwned::Static(PageTableStatic { table })
        }
    }

    pub unsafe fn unchecked_set_table(&mut self, table: MaybeOwnedTable<L>) -> &mut Self {
        let (addr, owned) = match table {
            MaybeOwned::Owned(table) => (table.leak().table as u64, true),
            MaybeOwned::Static(table) => (table.table as u64, false),
        };
        self.entry.set_larger_pages(false);
        self.entry.set_address(addr);
        self.entry.set_present(true);
        self.entry.set_fx_owned(owned);
        self
    }
}

impl<L: TableLevel> DirectoryEntry<L>
where
    L::Next: TableLevel,
    PageTable<L::Next>: TableOperations,
    L: TableLevel<Map = Infallible>,
{
    pub fn table(&self) -> Option<&PageTable<L::Next>> {
        self.is_table().then(|| unsafe { self.unchecked_table() })
    }

    pub fn table_mut(&mut self) -> Option<&mut PageTable<L::Next>> {
        self.is_table().then(|| {
            assert!(self.fx_owned());
            unsafe { self.unchecked_table_mut() }
        })
    }

    pub fn table_alloc(
        &mut self,
        flags: VMMapFlags,
        alloc: impl PageAllocator,
    ) -> &mut PageTable<L::Next> {
        if !self.is_table() {
            self.set_table(MaybeOwned::Owned(PageTableOwned::new(alloc).unwrap()));
        }
        self.upgrade_flags(flags);
        unsafe { self.unchecked_table_mut() }
    }

    pub fn set_table(&mut self, table: MaybeOwnedTable<L>) -> &mut Self {
        self.take_table();
        assert!(!self.present());
        unsafe { self.unchecked_set_table(table) }
    }

    pub fn take_table(&mut self) -> Option<MaybeOwnedTable<L>> {
        self.is_table()
            .then(|| unsafe { self.unchecked_take_table() })
    }
}

impl<L: TableLevel> DirectoryEntry<L>
where
    L::Map: PageSize,
{
    pub fn is_page(&self) -> bool {
        self.present() && self.entry.larger_pages() == L::LARGER_PAGES
    }

    pub fn page(&self) -> Option<Page<L::Map>> {
        self.is_page().then(|| unsafe { self.unchecked_page() })
    }

    pub unsafe fn unchecked_page(&self) -> Page<L::Map> {
        Page::new(self.entry.get_address())
    }

    pub unsafe fn unchecked_take_page(&mut self) -> MaybeOwnedPage<L> {
        unsafe {
            let p = self.unchecked_page();
            let owned = self.entry.fx_owned();
            self.entry = PageDirectoryEntry::new();
            if owned {
                let p = p.as_size4kb().expect("we can't allocate non 4kb pages yet");
                MaybeOwned::Owned(AllocatedPage::from_raw(p, GlobalPageAllocator))
            } else {
                MaybeOwned::Static(p)
            }
        }
    }

    pub unsafe fn unchecked_set_page(&mut self, page: MaybeOwnedPage<L>) -> &mut Self {
        let (address, owned) = match page {
            MaybeOwned::Owned(page) => {
                assert!(L::Map::PAGE_SIZE == Size4KB::PAGE_SIZE);
                (page.into_raw().get_address(), true)
            }
            MaybeOwned::Static(page) => (page.get_address(), false),
        };
        self.entry.set_larger_pages(L::LARGER_PAGES);
        self.entry.set_address(address);
        self.entry.set_fx_owned(owned);
        self.entry.set_present(true);

        self
    }
}

impl<L: TableLevel<Next = Infallible>> DirectoryEntry<L>
where
    L::Map: PageSize,
{
    pub fn take_page(&mut self) -> Option<MaybeOwnedPage<L>> {
        (self.is_page()).then(|| unsafe { self.unchecked_take_page() })
    }

    pub fn set_page(&mut self, page: MaybeOwnedPage<L>) -> &mut Self {
        self.take_page();
        assert!(!self.present());
        unsafe { self.unchecked_set_page(page) }
    }
}

pub enum MaybeOwned<O, S> {
    Owned(O),
    Static(S),
}

impl<O, S> From<O> for MaybeOwned<O, S> {
    fn from(value: O) -> Self {
        MaybeOwned::Owned(value)
    }
}

type MaybeOwnedTable<L> =
    MaybeOwned<PageTableOwned<<L as TableLevel>::Next>, PageTableStatic<<L as TableLevel>::Next>>;

type MaybeOwnedPage<L> =
    MaybeOwned<AllocatedPage<GlobalPageAllocator>, Page<<L as TableLevel>::Map>>;

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum EntryType {
    Empty,
    Table,
    Page,
}

pub enum Entry<L: TableLevel>
where
    L::Next: TableLevel,
    L::Map: PageSize,
    PageTable<L::Next>: TableOperations,
{
    Empty,
    Table(MaybeOwned<PageTableOwned<L::Next>, PageTableStatic<L::Next>>),
    Page(MaybeOwned<AllocatedPage<GlobalPageAllocator>, Page<L::Map>>),
}

pub enum EntryRef<'a, L: TableLevel>
where
    L::Map: PageSize,
    L::Next: TableLevel,
{
    Empty,
    Table(&'a PageTable<L::Next>),
    Page(Page<L::Map>),
}

pub enum EntryMut<'a, L: TableLevel>
where
    L::Map: PageSize,
    L::Next: TableLevel,
{
    Empty,
    Table(&'a mut PageTable<L::Next>),
    Page(Page<L::Map>),
}

impl<L: TableLevel> DirectoryEntry<L>
where
    L::Map: PageSize,
    L::Next: TableLevel,
    PageTable<L::Next>: TableOperations,
{
    pub fn entry_type(&self) -> EntryType {
        if !self.present() {
            EntryType::Empty
        } else if self.is_table() {
            EntryType::Table
        } else if self.is_page() {
            EntryType::Page
        } else {
            panic!()
        }
    }

    pub fn entry<'a>(&'a self) -> EntryRef<'a, L> {
        unsafe {
            match self.entry_type() {
                EntryType::Empty => EntryRef::Empty,
                EntryType::Table => EntryRef::Table(self.unchecked_table()),
                EntryType::Page => EntryRef::Page(self.unchecked_page()),
            }
        }
    }

    pub fn entry_mut<'a>(&'a mut self) -> EntryMut<'a, L> {
        unsafe {
            match self.entry_type() {
                EntryType::Empty => EntryMut::Empty,
                EntryType::Table if self.fx_owned() => EntryMut::Table(self.unchecked_table_mut()),
                EntryType::Page if self.fx_owned() => EntryMut::Page(self.unchecked_page()),
                _ => panic!("Attempt to get entry mut on non owned"),
            }
        }
    }

    pub fn take_entry(&mut self) -> Entry<L> {
        unsafe {
            match self.entry_type() {
                EntryType::Empty => Entry::Empty,
                EntryType::Table => Entry::Table(self.unchecked_take_table()),
                EntryType::Page => Entry::Page(self.unchecked_take_page()),
            }
        }
    }

    pub fn set_entry(&mut self, entry: Entry<L>) -> &mut Self {
        self.take_entry();
        assert!(!self.present());
        unsafe {
            match entry {
                Entry::Empty => self,
                Entry::Table(table) => self.unchecked_set_table(table),
                Entry::Page(page) => self.unchecked_set_page(page),
            }
        }
    }

    pub fn try_table(
        &mut self,
        flags: VMMapFlags,
        alloc: impl PageAllocator,
    ) -> Option<&mut PageTable<L::Next>> {
        unsafe {
            match self.entry_type() {
                EntryType::Empty => {
                    self.unchecked_set_table(MaybeOwned::Owned(
                        PageTableOwned::new(alloc).unwrap(),
                    ));
                    self.set_flags(flags);
                    Some(self.unchecked_table_mut())
                }
                EntryType::Table => {
                    self.upgrade_flags(flags);
                    Some(self.unchecked_table_mut())
                }
                EntryType::Page => None,
            }
        }
    }

    pub fn gc_table(&mut self) {
        match self.entry_type() {
            EntryType::Empty => (),
            EntryType::Table => unsafe {
                if self.unchecked_table().all_entries_empty() {
                    self.unchecked_take_table();
                }
            },
            EntryType::Page => panic!(),
        }
    }
}

impl PageTable<TableLevel4> {
    pub fn get_page_addresses(&self, mut f: impl FnMut(usize)) {
        for e in &self.entries {
            let Some(t) = e.table() else {
                continue;
            };
            f(e.entry.get_address() as usize);
            for e in &t.entries {
                let EntryRef::Table(t) = e.entry() else {
                    continue;
                };
                f(e.entry.get_address() as usize);
                for e in &t.entries {
                    let EntryRef::Table(_) = e.entry() else {
                        continue;
                    };
                    f(e.entry.get_address() as usize);
                }
            }
        }
    }
}

pub trait TableOperations {
    fn clear(&mut self);
    fn address_of(&self, vaddr: usize) -> Option<usize>;
}

impl TableOperations for PageTable<TableLevel1> {
    fn clear(&mut self) {
        for e in &mut self.entries {
            e.take_page();
        }
    }

    fn address_of(&self, vaddr: usize) -> Option<usize> {
        self.get(vaddr)
            .page()
            .map(|p| p.get_address() as usize + (vaddr % Size4KB::PAGE_SIZE as usize))
    }
}

macro_rules! gen_table_entry {
    ($table:ident) => {
        impl TableOperations for PageTable<$table> {
            fn clear(&mut self) {
                for e in &mut self.entries {
                    e.take_entry();
                }
            }

            fn address_of(&self, vaddr: usize) -> Option<usize> {
                match self.get(vaddr).entry() {
                    EntryRef::Empty => None,
                    EntryRef::Table(page_table) => page_table.address_of(vaddr),
                    EntryRef::Page(page) => Some(
                        page.get_address() as usize
                            + (vaddr % <$table as TableLevel>::Map::PAGE_SIZE as usize),
                    ),
                }
            }
        }
    };
}

gen_table_entry!(TableLevel2);
gen_table_entry!(TableLevel3);

impl TableOperations for PageTable<TableLevel4> {
    fn clear(&mut self) {
        for e in &mut self.entries {
            e.take_table();
        }
    }

    fn address_of(&self, vaddr: usize) -> Option<usize> {
        self.get(vaddr).table().and_then(|t| t.address_of(vaddr))
    }
}

impl PageTableOwned<TableLevel4> {
    pub fn new_with_global(alloc: impl PageAllocator) -> Option<Self> {
        let mut this = Self::new(alloc)?;

        let mut set = |addr, t: PageTableStatic<TableLevel3>| {
            this.as_mut()
                .get_mut(addr)
                .set_table(MaybeOwnedTable::<TableLevel4>::Static(t))
                .set_flags(VMMapFlags::WRITEABLE);
        };
        let ofm = OFFSET_MAP.lock().clone();
        let kdm = KERNEL_DATA_MAP.lock().clone();
        let khm = KERNEL_HEAP_MAP.lock().clone();
        let pcm = PER_CPU_MAP.lock().clone();
        let ksm = KERNEL_STACKS_MAP.lock().clone();
        set(MemoryLoc::PhysMapOffset as usize, ofm);
        set(MemoryLoc::KernelStart as usize, kdm);
        set(MemoryLoc::KernelHeap as usize, khm);
        set(MemoryLoc::PerCpuMem as usize, pcm);
        set(MemoryLoc::KernelStacks as usize, ksm);
        Some(this)
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
