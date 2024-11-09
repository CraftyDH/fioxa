use core::alloc::{GlobalAlloc, Layout};

use crate::{
    locked_mutex::Locked,
    paging::{
        get_uefi_active_mapper,
        page_allocator::{free_page_early, request_page},
        page_table_manager::{Mapper, Page, Size4KB},
        MemoryMappingFlags,
    },
    scheduling::with_held_interrupts,
};

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
    base_address: u64,
}

impl SlabAllocator {
    pub const fn new(base_address: u64) -> Self {
        const EMPTY: Option<&'static mut ListNode> = None;

        Self {
            slab_heads: [EMPTY; SLAB_SIZES.len()],
            base_address,
        }
    }
}

unsafe impl GlobalAlloc for Locked<SlabAllocator> {
    unsafe fn alloc(&self, layout: Layout) -> *mut u8 {
        with_held_interrupts(|| {
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
                        let frame = request_page().unwrap().leak();

                        let base = allocator.base_address;
                        allocator.base_address += 0x1000;

                        let mut mapper = get_uefi_active_mapper();
                        mapper
                            .map_memory(Page::new(base), frame, MemoryMappingFlags::WRITEABLE)
                            .unwrap()
                            .flush();

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
                None => {
                    // Round to next page
                    let length = ((min_size + 0xFFF) & !0xFFF) as u64;

                    let base = allocator.base_address;
                    allocator.base_address += length;

                    let mut mapper = get_uefi_active_mapper();
                    for page in (base..(base + length)).step_by(0x1000) {
                        let frame = request_page().unwrap().leak();

                        mapper
                            .map_memory(Page::new(page), frame, MemoryMappingFlags::WRITEABLE)
                            .unwrap()
                            .flush();
                    }
                    base as *mut u8
                }
            }
        })
    }

    unsafe fn dealloc(&self, ptr: *mut u8, layout: Layout) {
        with_held_interrupts(|| {
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
                    let base = ptr as u64;
                    let length = ((max_size + 0xFFF) & !0xFFF) as u64;

                    let mut mapper = get_uefi_active_mapper();
                    // We assume that we allocted cont pages
                    for page in (base..(base + length)).step_by(0x1000) {
                        let phys_page =
                            Page::new(mapper.get_phys_addr(Page::<Size4KB>::new(page)).unwrap());
                        mapper
                            .unmap_memory(Page::<Size4KB>::new(page))
                            .unwrap()
                            .flush();
                        free_page_early(phys_page);
                    }
                }
            }
        });
    }
}
