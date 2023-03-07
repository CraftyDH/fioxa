#![no_std]
#![feature(alloc_error_handler)] // We need to be able to create the error handler

use bump::BumpAllocator;
use locked_mutex::Locked;

pub mod bump;
pub mod locked_mutex;

extern crate alloc;

#[alloc_error_handler]
fn alloc_error_handler(layout: alloc::alloc::Layout) -> ! {
    panic!("Allocation Error: {:?}", layout)
}

#[global_allocator]
static ALLOCATOR: Locked<BumpAllocator> = Locked::new(BumpAllocator::new());
