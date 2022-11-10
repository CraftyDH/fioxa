use lazy_static::lazy_static;
use x86_64::structures::tss::TaskStateSegment;
use x86_64::VirtAddr;

use crate::paging::page_allocator::frame_alloc_exec;

pub const DOUBLE_FAULT_IST_INDEX: u16 = 0;
pub const PAGE_FAULT_IST_INDEX: u16 = 1;
pub const TASK_SWITCH_INDEX: u16 = 2;

lazy_static! {
    pub static ref TSS: [TaskStateSegment; 8] = {
        core::array::from_fn(|_| {
            let mut tss = TaskStateSegment::new();
            tss.interrupt_stack_table[DOUBLE_FAULT_IST_INDEX as usize] = {
                const STACK_SIZE: usize = 0x1000 * 5;
                let stack_base = frame_alloc_exec(|c| c.lock().request_cont_pages(5)).unwrap();

                let stack_start = VirtAddr::from_ptr(stack_base as *mut u8);
                let stack_end = stack_start + STACK_SIZE;
                stack_end
            };
            tss.interrupt_stack_table[PAGE_FAULT_IST_INDEX as usize] = {
                const STACK_SIZE: usize = 0x1000 * 5;
                let stack_base = frame_alloc_exec(|c| c.lock().request_cont_pages(5)).unwrap();

                let stack_start = VirtAddr::from_ptr(stack_base as *mut u8);
                let stack_end = stack_start + STACK_SIZE;
                stack_end
            };
            tss.interrupt_stack_table[TASK_SWITCH_INDEX as usize] = {
                const STACK_SIZE: usize = 0x1000 * 5;
                let stack_base = frame_alloc_exec(|c| c.lock().request_cont_pages(5)).unwrap();

                let stack_start = VirtAddr::from_ptr(stack_base as *mut u8);
                let stack_end = stack_start + STACK_SIZE;
                stack_end
            };

            tss
        })
    };
}
