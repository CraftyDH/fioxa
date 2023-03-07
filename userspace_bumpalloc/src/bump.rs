use core::{
    alloc::{GlobalAlloc, Layout},
    ptr,
};

use userspace::syscall::mmap_page;

use crate::locked_mutex::Locked;

/// Align downwards. Returns the greatest x with alignment `align`
/// so that x <= addr. The alignment must be a power of 2.
pub fn align_down(addr: usize, align: usize) -> usize {
    if align.is_power_of_two() {
        addr & !(align - 1)
    } else if align == 0 {
        addr
    } else {
        panic!("`align` must be a power of 2");
    }
}

/// Align upwards. Returns the smallest x with alignment `align`
/// so that x >= addr. The alignment must be a power of 2.
pub fn align_up(addr: usize, align: usize) -> usize {
    align_down(addr + align - 1, align)
}

pub struct BumpAllocator {
    heap_start: usize,
    heap_end: usize,
    next: usize,
    allocations: usize,
}

impl BumpAllocator {
    // Create a new Bump Allocator
    pub const fn new() -> Self {
        Self {
            heap_start: 0x7F0000000000,
            heap_end: 0x7F0000000000,
            next: 0x7F0000000000,
            allocations: 0,
        }
    }
}

unsafe impl GlobalAlloc for Locked<BumpAllocator> {
    unsafe fn alloc(&self, layout: Layout) -> *mut u8 {
        let mut bump = self.lock();

        if bump.heap_start == 0 {
            return ptr::null_mut();
        }

        let alloc_start = align_up(bump.next, layout.align());
        let alloc_end = match alloc_start.checked_add(layout.size()) {
            Some(end) => end,
            None => return ptr::null_mut(),
        };

        if alloc_end > bump.heap_end {
            for page in (bump.heap_end..alloc_end + 0xFFF).step_by(0x1000) {
                mmap_page(page)
            }
        }

        // Increment variables to reflect to allocated block
        bump.next = alloc_end;
        bump.heap_end = align_up(alloc_end, 0x1000);
        bump.allocations += 1;
        alloc_start as *mut u8
    }

    unsafe fn dealloc(&self, _ptr: *mut u8, _layout: Layout) {
        let mut bump = self.lock();

        bump.allocations -= 1;

        // If no more allocation restart pool at 0
        if bump.allocations == 0 {
            bump.next = bump.heap_start;
        }
    }
}
