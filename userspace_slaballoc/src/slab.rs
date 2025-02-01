use core::{
    alloc::{GlobalAlloc, Layout},
    ptr::null_mut,
};

use kernel_sys::{
    syscall::{sys_map, sys_unmap},
    types::MapMemoryFlags,
};

use crate::locked_mutex::Locked;

const SLAB_SIZES: &[usize] = &[8, 16, 32, 64, 128, 256, 512, 1024, 2048];

fn block_size(min_size: usize) -> Option<usize> {
    // Find smallest block
    SLAB_SIZES.iter().position(|&size| size >= min_size)
}

struct ListNode {
    next: Option<&'static mut ListNode>,
}

pub struct SlabAllocator {
    slab_heads: [Option<&'static mut ListNode>; SLAB_SIZES.len()],
}

impl SlabAllocator {
    pub const fn new() -> Self {
        const EMPTY: Option<&'static mut ListNode> = None;

        Self {
            slab_heads: [EMPTY; SLAB_SIZES.len()],
        }
    }
}

unsafe impl GlobalAlloc for Locked<SlabAllocator> {
    unsafe fn alloc(&self, layout: Layout) -> *mut u8 {
        let mut allocator = self.lock();

        let min_size = layout.size().max(layout.align());

        match block_size(min_size) {
            Some(index) => match allocator.slab_heads[index].take() {
                Some(node) => {
                    allocator.slab_heads[index] = node.next.take();
                    node as *mut ListNode as *mut u8
                }
                None => {
                    // No block exists in list => allocate new block
                    let block_size = SLAB_SIZES[index];
                    // Only works if all blocks are powers of 2

                    let base =
                        sys_map(null_mut(), 0x1000, MapMemoryFlags::WRITEABLE).unwrap() as usize;

                    let mut current_node = None;
                    for block in (base..(base + 0x1000)).step_by(block_size) {
                        let node = &mut *(block as *mut ListNode);
                        node.next = current_node;
                        current_node = Some(node);
                    }
                    let nxt = current_node.take().unwrap();
                    allocator.slab_heads[index] = nxt.next.take();
                    // current_node as *mut ListNode as *mut u8
                    nxt as *mut ListNode as *mut u8
                }
            },
            None => sys_map(null_mut(), min_size, MapMemoryFlags::WRITEABLE)
                .unwrap()
                .cast(),
        }
    }

    unsafe fn dealloc(&self, ptr: *mut u8, layout: Layout) {
        let mut allocator = self.lock();

        let max_size = layout.size().max(layout.align());

        match block_size(max_size) {
            Some(index) => {
                // Return block to correct list size
                let new_node = ListNode {
                    next: allocator.slab_heads[index].take(),
                };

                let new_node_ptr = ptr as *mut ListNode;
                new_node_ptr.write(new_node);
                allocator.slab_heads[index] = Some(&mut *new_node_ptr);
            }
            None => {
                sys_unmap(ptr.cast(), max_size).assert_ok();
            }
        }
    }
}
