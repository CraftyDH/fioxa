#![no_std]
#![feature(alloc_error_handler)] // We need to be able to create the error handler
#![feature(const_mut_refs)]

use locked_mutex::Locked;
use slab::SlabAllocator;

pub mod locked_mutex;
pub mod slab;

extern crate alloc;

#[alloc_error_handler]
fn alloc_error_handler(layout: alloc::alloc::Layout) -> ! {
    panic!("Allocation Error: {:?}", layout)
}

#[global_allocator]
static ALLOCATOR: Locked<SlabAllocator> = Locked::new(SlabAllocator::new());
