use core::{marker::PhantomData, num::NonZeroU64};

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

    // HACK to get around generics
    pub fn as_size4kb(self) -> Option<Page<Size4KB>> {
        (S::PAGE_SIZE == Size4KB::PAGE_SIZE).then_some(Page {
            address: self.address,
            _size: PhantomData,
        })
    }
}

impl<S: PageSize> Copy for Page<S> {}

impl<S: PageSize> Clone for Page<S> {
    fn clone(&self) -> Self {
        *self
    }
}

pub struct ChunkedPageRange {
    pub lower_align_4kb: PageRange<Size4KB>,
    pub lower_align_2mb: PageRange<Size2MB>,
    pub middle: PageRange<Size1GB>,
    pub upper_align_2mb: PageRange<Size2MB>,
    pub upper_align_4kb: PageRange<Size4KB>,
}

pub fn get_chunked_page_range(mut start: u64, mut end: u64) -> ChunkedPageRange {
    assert!(start & 0xFFF == 0);
    assert!(end & 0xFFF == 0);

    // Normalize 4kb chunks
    let lower_align_4kb = if start & 0x1f_ffff > 0 {
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

    let upper_align_4kb = if end & 0x1f_ffff > 0 && start != end {
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
    let lower_align_2mb = if start & 0x3fffffff > 0 && start != end {
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

    let upper_align_2mb = if end & 0x3fffffff > 0 && start != end {
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

    let middle = if start < end && start != end {
        PageRange::new(start, ((end - start) / Size1GB::PAGE_SIZE) as usize)
    } else {
        PageRange::empty()
    };

    ChunkedPageRange {
        lower_align_4kb,
        lower_align_2mb,
        middle,
        upper_align_2mb,
        upper_align_4kb,
    }
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
