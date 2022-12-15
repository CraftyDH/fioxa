pub mod lvl1;
pub mod lvl2;
pub mod lvl3;
pub mod lvl4;

use core::marker::PhantomData;

use super::{
    get_uefi_active_mapper,
    page_allocator::{free_page, request_page},
    page_directory::PageDirectoryEntry,
    virt_addr_for_phys, MemoryLoc,
};

pub trait PageSize: Sized {
    fn size() -> u64;
}
#[derive(Debug)]
pub struct Size4KB;
#[derive(Debug)]

pub struct Size2MB;
#[derive(Debug)]
pub struct Size1GB;

impl PageSize for Size4KB {
    fn size() -> u64 {
        0x1000
    }
}
impl PageSize for Size2MB {
    fn size() -> u64 {
        0x200000
    }
}
impl PageSize for Size1GB {
    fn size() -> u64 {
        0x40000000
    }
}

pub struct Page<S: PageSize> {
    address: u64,
    _size: core::marker::PhantomData<S>,
}

impl<S: PageSize> Page<S> {
    pub fn get_address(&self) -> u64 {
        self.address
    }

    pub fn new(address: u64) -> Self {
        assert!(address & (S::size() - 1) == 0);

        let lvl4 = (address >> (12 + 9 + 9 + 9)) & 0x1ff;
        let sign = address >> (12 + 9 + 9 + 9 + 9);

        assert!(
            (sign == 0 && lvl4 <= 255) || (sign == 0xFFFF && lvl4 > 255),
            "Sign extension is not valid, addr: {:#X}, sign: {:#X}, lvl4: {}",
            address,
            sign,
            lvl4,
        );

        Page {
            address,
            _size: core::marker::PhantomData,
        }
    }
}

impl Copy for Page<Size4KB> {}

impl Clone for Page<Size4KB> {
    fn clone(&self) -> Self {
        Self {
            address: self.address.clone(),
            _size: core::marker::PhantomData,
        }
    }
}

pub const fn page_4kb(address: u64) -> Page<Size4KB> {
    assert!(address & 0xFFF == 0);
    Page {
        address,
        _size: core::marker::PhantomData,
    }
}

pub trait Mapper<S: PageSize> {
    fn map_memory(&mut self, from: Page<S>, to: Page<S>) -> Option<Flusher>;
    fn unmap_memory(&mut self, page: Page<S>) -> Option<Flusher>;
    fn get_phys_addr(&self, page: Page<S>) -> Option<u64>;
}

pub trait PageLevel {}
pub struct PageLvl1;
pub struct PageLvl2;
pub struct PageLvl3;
pub struct PageLvl4;

impl PageLevel for PageLvl1 {}
impl PageLevel for PageLvl2 {}
impl PageLevel for PageLvl3 {}
impl PageLevel for PageLvl4 {}

#[repr(C, align(0x1000))]
pub struct PhysPageTable {
    entries: [PageDirectoryEntry; 512],
}

impl PhysPageTable {
    fn get_or_create_table(&mut self, idx: usize) -> &mut PhysPageTable {
        let entry = &mut self.entries[idx as usize];
        if entry.larger_pages() {
            panic!("Page Lvl4 cannot contain huge pages")
        }
        let addr;
        if entry.present() {
            addr = virt_addr_for_phys(entry.get_address()) as *mut PhysPageTable;
        } else {
            let new_page = request_page().unwrap();
            addr = virt_addr_for_phys(new_page) as *mut PhysPageTable;

            entry.set_address(new_page);
            entry.set_present(true);
            entry.set_read_write(true);
            entry.set_user_super(true);
        }

        unsafe { &mut *addr }
    }

    fn set_table(&mut self, idx: usize, table: &mut PhysPageTable) {
        let entry = &mut self.entries[idx as usize];
        if entry.present() {
            println!("WARN: setting table over existing entry");
        }
        entry.set_address(table.entries.as_ptr() as u64);
        entry.set_present(true);
        entry.set_read_write(true);
        entry.set_user_super(true);
    }

    fn get_table(&self, idx: usize) -> Option<&mut PhysPageTable> {
        let entry = &self.entries[idx as usize];
        if entry.larger_pages() {
            panic!("Page Lvl4 cannot contain huge pages")
        }
        if entry.present() {
            Some(unsafe { &mut *(virt_addr_for_phys(entry.get_address()) as *mut PhysPageTable) })
        } else {
            None
        }
    }
}

pub struct PageTable<'t, L: PageLevel> {
    table: &'t mut PhysPageTable,
    level: core::marker::PhantomData<L>,
}

pub unsafe fn new_page_table_from_phys<L: PageLevel>(addr: u64) -> PageTable<'static, L> {
    let table = virt_addr_for_phys(addr) as *mut PhysPageTable;

    PageTable {
        table: unsafe { &mut *table },
        level: core::marker::PhantomData,
    }
}

pub fn ident_map_curr_process(memory: u64, write: bool) {
    let mut mapper = unsafe { get_uefi_active_mapper() };
    let page = page_4kb(memory);
    mapper.map_memory(page, page).unwrap().flush();
}
#[must_use = "TLB must be flushed or can be ignored"]
pub struct Flusher(u64);

impl Flusher {
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
        let tmp = PageRange {
            idx: start,
            end: new_start,
            _size: PhantomData,
        };
        start = new_start;
        tmp
    } else {
        PageRange {
            idx: 1,
            end: 0,
            _size: PhantomData,
        }
    };

    let e4kb = if end & 0x1f_ffff > 0 && start != end {
        let new_end = core::cmp::max(end & !0x1f_ffff, start);
        let tmp = PageRange {
            idx: new_end,
            end: end,
            _size: PhantomData,
        };
        end = new_end;
        tmp
    } else {
        PageRange {
            idx: 1,
            end: 0,
            _size: PhantomData,
        }
    };

    // Normalize 2mb chunks
    let s2mb = if start & 0x3fffffff > 0 && start != end {
        let new_start = core::cmp::min((start & !0x3fffffff) + 0x40000000, end);
        let tmp = PageRange {
            idx: start,
            end: new_start,
            _size: PhantomData,
        };
        start = new_start;
        tmp
    } else {
        PageRange {
            idx: 1,
            end: 0,
            _size: PhantomData,
        }
    };

    let e2mb = if end & 0x3fffffff > 0 && start != end {
        let new_end = core::cmp::max(end & !0x3fffffff, start);

        let tmp = PageRange {
            idx: new_end,
            end: end,
            _size: PhantomData,
        };
        end = new_end;
        tmp
    } else {
        PageRange {
            idx: 1,
            end: 0,
            _size: PhantomData,
        }
    };

    let gb = if start < end && start != end {
        PageRange {
            idx: start,
            end,
            _size: PhantomData,
        }
    } else {
        PageRange {
            idx: 1,
            end: 0,
            _size: PhantomData,
        }
    };

    (s4kb, s2mb, gb, e2mb, e4kb)
}

#[derive(Debug)]
pub struct PageRange<S: PageSize> {
    idx: u64,
    end: u64,
    _size: PhantomData<S>,
}

impl<S: PageSize> Iterator for PageRange<S> {
    type Item = Page<S>;

    fn next(&mut self) -> Option<Self::Item> {
        let res = self.idx;
        self.idx += S::size();

        if res < self.end {
            Some(Page {
                address: res,
                _size: PhantomData,
            })
        } else {
            None
        }
    }
}
