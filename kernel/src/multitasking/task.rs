use core::{convert::TryInto, sync::atomic::Ordering};

use x86_64::{
    structures::idt::{InterruptStackFrame, InterruptStackFrameValue},
    VirtAddr,
};

use crate::{
    assembly::registers::Registers,
    gdt::GDT,
    paging::{page_allocator::request_page, page_table_manager::PageTableManager},
};

use super::{taskmanager::THREAD_BOOTSTRAPER, Task, TaskID, STACK_ADDR, STACK_SIZE};

impl Task {
    pub fn new(mapper: &mut PageTableManager) -> Self {
        // Allow 1 page difference to catch stack overflow
        let stack_base =
            STACK_ADDR.fetch_add((STACK_SIZE + 0x1000).try_into().unwrap(), Ordering::Relaxed);

        for addr in (stack_base..(stack_base + STACK_SIZE as u64)).step_by(0x1000) {
            let frame = request_page().unwrap();

            mapper.map_memory(addr, frame as u64);
        }

        mapper.flush_cr3();

        let state_isf = InterruptStackFrameValue {
            instruction_pointer: *THREAD_BOOTSTRAPER,
            code_segment: GDT.1.code_selector.0 as u64,
            cpu_flags: 0x202,
            // cpu_flags: (RFlags::IOPL_HIGH | RFlags::IOPL_LOW | RFlags::INTERRUPT_FLAG).bits(),
            stack_pointer: VirtAddr::new(stack_base + STACK_SIZE as u64),
            stack_segment: 0,
        };

        Self {
            id: TaskID::new(),
            state_isf,
            state_reg: Registers::default(),
        }
    }

    pub fn save(&mut self, stack_frame: &mut InterruptStackFrame, regs: &mut Registers) {
        self.state_isf = stack_frame.clone();
        self.state_reg = regs.clone();
    }
}
