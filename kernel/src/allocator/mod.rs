use crate::{locked_mutex::Locked, paging::MemoryLoc};

use self::slab_allocator::SlabAllocator;

// pub const HEAP_START: usize = 0xFFFFFFFE00000000;
// const HEAP_START: usize = MemoryLoc::KernelHeap as usize;
// pub const HEAP_SIZE: usize = 1024 * 1024 * 50; // 50 MiB

// pub mod bump;
// pub mod fixed_size_block;
// pub mod linked_list;
pub mod slab_allocator;

// pub fn init_heap(mapper: &mut PageTable<PageLvl4>) -> Result<(), MapToError<Size4KiB>> {
//     for page in (HEAP_START..(HEAP_START + HEAP_SIZE - 1)).step_by(0x1000) {
//         let frame = request_page().ok_or(MapToError::FrameAllocationFailed)?;
//         // let flags = PageTableFlags::PRESENT | PageTableFlags::WRITABLE;
//         mapper
//             .map_memory(page_4kb(page as u64), page_4kb(frame as u64))
//             .unwrap()
//             .flush();
//     }

//     unsafe {
//         ALLOCATOR.lock().init(HEAP_START, HEAP_SIZE);
//     }

//     Ok(())
// }

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
// #[global_allocator]
// pub static ALLOCATOR: Locked<FixedSizeBlockAllocator> = Locked::new(FixedSizeBlockAllocator::new());

#[global_allocator]
pub static ALLOCATOR: Locked<SlabAllocator> =
    Locked::new(SlabAllocator::new(MemoryLoc::KernelHeap as u64));
