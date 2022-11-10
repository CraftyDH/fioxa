use x86_64::structures::paging::{mapper::MapToError, Size4KiB};

use crate::{
    locked_mutex::Locked,
    paging::{page_allocator::request_page, page_table_manager::PageTableManager},
};

use self::fixed_size_block::FixedSizeBlockAllocator;

pub const HEAP_START: usize = 0x40_000_000;
pub const HEAP_SIZE: usize = 1024 * 1024 * 5; // 5 MiB

pub mod bump;
pub mod fixed_size_block;
pub mod linked_list;

pub fn init_heap(mapper: &mut PageTableManager) -> Result<(), MapToError<Size4KiB>> {
    for page in (HEAP_START..(HEAP_START + HEAP_SIZE - 1)).step_by(0x1000) {
        let frame = request_page().ok_or(MapToError::FrameAllocationFailed)?;
        // let flags = PageTableFlags::PRESENT | PageTableFlags::WRITABLE;
        mapper
            .map_memory(page as u64, frame as u64, true)
            .unwrap()
            .flush();
    }

    unsafe {
        ALLOCATOR.lock().init(HEAP_START, HEAP_SIZE);
    }

    Ok(())
}

//* Use LockedHeap allocator crate
// #[global_allocator]
// static ALLOCATOR: LockedHeap = LockedHeap::empty();

//* Use bump allocator
// #[global_allocator]
// static ALLOCATOR: Locked<BumpAllocator> = Locked::new(BumpAllocator::new());

//* Use Linked List
// #[global_allocator]
// static ALLOCATOR: Locked<LinkedListAllocator> = Locked::new(LinkedListAllocator::new());

//* Use Fixed Block Sizes
#[global_allocator]
pub static ALLOCATOR: Locked<FixedSizeBlockAllocator> = Locked::new(FixedSizeBlockAllocator::new());
