use core::{convert::TryInto, sync::atomic::Ordering};

use x86_64::{
    structures::{
        idt::{InterruptStackFrame, InterruptStackFrameValue},
        paging::{FrameAllocator, Mapper, OffsetPageTable, Page, PageTableFlags, Size4KiB},
    },
    VirtAddr,
};

use crate::{assembly::registers::Registers, gdt::GDT, memory::uefi::FRAME_ALLOCATOR};

use super::{Task, TaskID, STACK_ADDR, STACK_SIZE};

impl Task {
    pub fn new(mapper: &mut OffsetPageTable<'static>) -> Self {
        // Allocate a new frame to store the stack in
        let frame = FRAME_ALLOCATOR.lock().allocate_frame().unwrap();

        // Add 4KB so that if process grows to far we get a page fault
        let addr = STACK_ADDR.fetch_add((STACK_SIZE + 4096).try_into().unwrap(), Ordering::SeqCst);
        let page = Page::<Size4KiB>::containing_address(VirtAddr::new(addr));
        let flags = PageTableFlags::PRESENT | PageTableFlags::WRITABLE;

        // Map the frame the virtual stack address
        // ! Ignore mapping as we have identity mapped the entire memory
        // fail_mapper(mapper, addr);
        unsafe {
            mapper
                .map_to(page, frame, flags, &mut *FRAME_ALLOCATOR.lock())
                .unwrap()
                .flush();
        }

        println!("mapped success ?: {:?}", mapper.translate_page(page));

        let state_isf = InterruptStackFrameValue {
            instruction_pointer: VirtAddr::new(10),
            code_segment: GDT.1.code_selector.0 as u64,
            cpu_flags: 0x202,
            // cpu_flags: (RFlags::IOPL_HIGH | RFlags::IOPL_LOW | RFlags::INTERRUPT_FLAG).bits(),
            stack_pointer: VirtAddr::new(addr + STACK_SIZE as u64),
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
