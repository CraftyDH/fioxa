use core::sync::atomic::{AtomicU64, Ordering};

use alloc::{
    boxed::Box,
    collections::{BTreeMap, VecDeque},
    string::String,
    sync::Arc,
    vec::Vec,
};
use kernel_userspace::syscall::exit;
use x86_64::{
    structures::{
        gdt::SegmentSelector,
        idt::{InterruptStackFrame, InterruptStackFrameValue},
    },
    VirtAddr,
};

use crate::{
    assembly::registers::Registers,
    gdt,
    paging::{
        offset_map::map_gop,
        page_allocator::request_page,
        page_table_manager::{new_page_table_from_phys, page_4kb, Mapper, PageLvl4, PageTable},
        MemoryLoc, KERNEL_DATA_MAP, KERNEL_HEAP_MAP, OFFSET_MAP, PER_CPU_MAP,
    },
    service::KernelMessageHeader,
};

const STACK_ADDR: u64 = 0x100_000_000_000;

const STACK_SIZE: u64 = 1024 * 512;

#[derive(Debug, PartialEq, Eq, PartialOrd, Ord, Clone, Copy)]
pub struct PID(pub u64);

impl PID {
    pub fn nop() -> Self {
        PID(0)
    }
}

impl Into<u64> for PID {
    fn into(self) -> u64 {
        self.0
    }
}

impl Into<usize> for PID {
    fn into(self) -> usize {
        self.0 as usize
    }
}

impl From<u64> for PID {
    fn from(value: u64) -> Self {
        PID(value)
    }
}

#[derive(Debug, PartialEq, Eq, PartialOrd, Ord, Clone, Copy)]
pub struct TID(u64);

impl PID {
    pub fn new() -> Self {
        static NEXT_ID: AtomicU64 = AtomicU64::new(0);
        Self(NEXT_ID.fetch_add(1, Ordering::Relaxed))
    }
}

impl Into<u64> for TID {
    fn into(self) -> u64 {
        self.0
    }
}

impl Into<usize> for TID {
    fn into(self) -> usize {
        self.0 as usize
    }
}

impl From<u64> for TID {
    fn from(value: u64) -> Self {
        TID(value)
    }
}

pub enum ProcessPrivilige {
    KERNEL,
    USER,
}

impl ProcessPrivilige {
    pub fn get_code_segment(&self) -> SegmentSelector {
        match self {
            ProcessPrivilige::KERNEL => gdt::KERNEL_CODE_SELECTOR,
            ProcessPrivilige::USER => gdt::USER_CODE_SELECTOR,
        }
    }

    pub fn get_data_segment(&self) -> SegmentSelector {
        match self {
            ProcessPrivilige::KERNEL => gdt::KERNEL_DATA_SELECTOR,
            ProcessPrivilige::USER => gdt::USER_DATA_SELECTOR,
        }
    }
}

pub struct Process {
    pub pid: PID,
    pub threads: BTreeMap<TID, Thread>,
    pub page_mapper: PageTable<'static, PageLvl4>,
    privilege: ProcessPrivilige,
    thread_next_id: u64,
    pub args: Vec<u8>,
    pub service_msgs: VecDeque<Arc<KernelMessageHeader>>,
}

impl Process {
    pub fn new(privilege: ProcessPrivilige, args: &[u8]) -> Self {
        let pml4 = request_page().unwrap();
        let mut page_mapper = unsafe { new_page_table_from_phys(pml4) };

        unsafe {
            page_mapper.set_lvl3_location(MemoryLoc::PhysMapOffset as u64, &mut *OFFSET_MAP.lock());
            page_mapper
                .set_lvl3_location(MemoryLoc::KernelStart as u64, &mut *KERNEL_DATA_MAP.lock());
            page_mapper
                .set_lvl3_location(MemoryLoc::KernelHeap as u64, &mut *KERNEL_HEAP_MAP.lock());
            page_mapper.set_lvl3_location(MemoryLoc::PerCpuMem as u64, &mut *PER_CPU_MAP.lock());
            map_gop(&mut page_mapper);
            page_mapper.map_memory(page_4kb(0xfee000b0 & !0xFFF), page_4kb(0xfee000b0 & !0xFFF));
        }

        Self {
            pid: PID::new(),
            threads: Default::default(),
            page_mapper,
            privilege,
            thread_next_id: 0,
            args: args.to_vec(),
            service_msgs: Default::default(),
        }
    }

    pub unsafe fn new_with_page(
        privilege: ProcessPrivilige,
        page_mapper: PageTable<'static, PageLvl4>,
        args: String,
    ) -> Self {
        Self {
            pid: PID::new(),
            threads: Default::default(),
            page_mapper,
            privilege,
            thread_next_id: 0,
            args: Vec::new(),
            service_msgs: Default::default(),
        }
    }

    // A in place thread which data will overriden with the real thread on its context switch out.
    pub unsafe fn new_overide_thread(&mut self) -> TID {
        let tid = TID(self.thread_next_id);
        self.thread_next_id += 1;

        let pushed_register_state = InterruptStackFrameValue {
            instruction_pointer: VirtAddr::new(0x8002),
            code_segment: 0,
            cpu_flags: 0,
            stack_pointer: VirtAddr::new(0),
            stack_segment: 0,
        };

        let register_state = Registers::default();

        let thread = Thread {
            tid,
            register_state,
            pushed_register_state,
            current_message: Default::default(),
        };

        self.threads.insert(tid, thread);
        tid
    }

    pub fn new_thread_direct(&mut self, entry_point: *const u64, register_state: Registers) -> TID {
        let tid = TID(self.thread_next_id);
        self.thread_next_id += 1;

        // let stack_base = STACK_ADDR.fetch_add(0x1000_000, Ordering::Relaxed);
        let stack_base = STACK_ADDR + (STACK_SIZE + 0x1000) * tid.0 as u64;

        for addr in (stack_base..(stack_base + STACK_SIZE as u64 - 1)).step_by(0x1000) {
            let frame = request_page().unwrap();

            self.page_mapper
                .map_memory(page_4kb(addr), page_4kb(frame))
                .unwrap()
                .flush();
        }

        let pushed_register_state = InterruptStackFrameValue {
            instruction_pointer: VirtAddr::from_ptr(entry_point),
            code_segment: self.privilege.get_code_segment().0 as u64,
            cpu_flags: 0x202,
            stack_pointer: VirtAddr::new(stack_base + STACK_SIZE as u64),
            stack_segment: self.privilege.get_data_segment().0 as u64,
        };

        let thread = Thread {
            tid,
            register_state,
            pushed_register_state,
            current_message: Default::default(),
        };

        self.threads.insert(tid, thread);
        tid
    }

    pub fn new_thread(&mut self, entry_point: usize) -> TID {
        let mut register_state = Registers::default();
        register_state.rdi = entry_point;

        self.new_thread_direct(thread_bootstraper as *const u64, register_state)
    }
}

pub struct Thread {
    pub tid: TID,
    pub register_state: Registers,
    // Rest of the data inclusing rip & rsp
    pub pushed_register_state: InterruptStackFrameValue,
    // Used for storing current msg, so that the popdata can get the data
    pub current_message: Option<Arc<KernelMessageHeader>>,
}

impl Thread {
    pub fn save(&mut self, stack_frame: &mut InterruptStackFrame, reg: &mut Registers) {
        self.pushed_register_state.clone_from(stack_frame);
        self.register_state.clone_from(reg);
    }
    pub fn restore(&self, stack_frame: &mut InterruptStackFrame, reg: &mut Registers) {
        unsafe {
            stack_frame
                .as_mut()
                .extract_inner()
                .clone_from(&self.pushed_register_state);
        }
        reg.clone_from(&self.register_state);
    }
}

extern "C" fn thread_bootstraper(main: usize) {
    // Recreate the function box that was passed from the syscall
    let func = unsafe { Box::from_raw(main as *mut Box<dyn FnOnce()>) };

    // Call the function
    func.call_once(());

    // Function ended quit
    exit()
}
