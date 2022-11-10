use core::sync::atomic::{AtomicU64, Ordering};

use alloc::{boxed::Box, collections::BTreeMap};
use x86_64::{
    structures::idt::{InterruptStackFrame, InterruptStackFrameValue},
    VirtAddr,
};

use crate::{
    assembly::registers::Registers,
    gdt::GDT,
    paging::{
        identity_map::FULL_IDENTITY_MAP, page_allocator::request_page,
        page_table_manager::PageTableManager,
    },
    syscall::exit_thread,
};

// const STACK_ADDR: AtomicU64 = AtomicU64::new(0x100_000_000_000);
const STACK_ADDR: u64 = 0x100_000_000_000;

const STACK_SIZE: u64 = 1024 * 512;

#[derive(Debug, PartialEq, Eq, PartialOrd, Ord, Clone, Copy)]
pub struct PID(u64);

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

pub struct Process {
    pub pid: PID,
    pub threads: BTreeMap<TID, Thread>,
    pub page_mapper: PageTableManager,
    thread_next_id: u64,
}

impl Process {
    pub fn new() -> Self {
        let pml4 = request_page().unwrap();
        let page_mapper = PageTableManager::new(pml4);

        Self {
            pid: PID::new(),
            threads: Default::default(),
            page_mapper,
            thread_next_id: 0,
        }
    }
    pub unsafe fn new_with_page(page_mapper: PageTableManager) -> Self {
        Self {
            pid: PID::new(),
            threads: Default::default(),
            page_mapper,
            thread_next_id: 0,
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
        };

        self.threads.insert(tid, thread);
        tid
    }

    pub fn new_thread(&mut self, entry_point: usize) -> TID {
        let tid = TID(self.thread_next_id);
        self.thread_next_id += 1;

        // let stack_base = STACK_ADDR.fetch_add(0x1000_000, Ordering::SeqCst);
        let stack_base = STACK_ADDR + (STACK_SIZE + 0x1000) * tid.0 as u64;

        for addr in (stack_base..(stack_base + STACK_SIZE as u64 - 1)).step_by(0x1000) {
            let frame = request_page().unwrap();

            self.page_mapper
                .map_memory(addr, frame, true)
                .unwrap()
                .flush();
            FULL_IDENTITY_MAP
                .lock()
                .map_memory(addr, frame, true)
                .unwrap()
                .flush();
        }

        let cs = GDT[0].1.code_selector.0 as u64;

        let pushed_register_state = InterruptStackFrameValue {
            instruction_pointer: *THREAD_BOOTSTRAPER,
            code_segment: cs,
            cpu_flags: 0x202,
            stack_pointer: VirtAddr::new(stack_base + STACK_SIZE as u64),
            stack_segment: 0,
        };

        let mut register_state = Registers::default();
        register_state.rdi = entry_point;

        let thread = Thread {
            tid,
            register_state,
            pushed_register_state,
        };

        self.threads.insert(tid, thread);
        tid
    }
}

pub struct Thread {
    pub tid: TID,
    pub register_state: Registers,
    // Rest of the data inclusing rip & rsp
    pub pushed_register_state: InterruptStackFrameValue,
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

use lazy_static::lazy_static;
lazy_static! {
    pub static ref THREAD_BOOTSTRAPER: VirtAddr =
        VirtAddr::from_ptr(thread_bootstraper as *mut usize);
}

extern "C" fn thread_bootstraper(main: usize) {
    // Recreate the function box that was passed from the syscall
    let func = unsafe { Box::from_raw(main as *mut Box<dyn FnOnce()>) };

    // Call the function
    func.call_once(());

    // Function ended quit
    exit_thread()
}
