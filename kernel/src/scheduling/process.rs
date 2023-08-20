use core::sync::atomic::{AtomicU64, Ordering};

use alloc::{
    boxed::Box,
    collections::{BTreeMap, VecDeque},
    sync::{Arc, Weak},
    vec::Vec,
};
use kernel_userspace::{
    ids::{ProcessID, ServiceID, ThreadID},
    service::ServiceTrackingNumber,
    syscall::exit,
};
use spin::Mutex;
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
        page_allocator::{request_page, AllocatedPage},
        page_table_manager::{Mapper, Page, PageLvl4, PageTable, Size4KB},
        MemoryLoc, KERNEL_DATA_MAP, KERNEL_HEAP_MAP, OFFSET_MAP, PER_CPU_MAP,
    },
};

const STACK_ADDR: u64 = 0x100_000_000_000;

const STACK_SIZE: u64 = 1024 * 12;

fn generate_next_process_id() -> ProcessID {
    static PID: AtomicU64 = AtomicU64::new(0);
    ProcessID(PID.fetch_add(1, Ordering::Relaxed))
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
    pub pid: ProcessID,
    pub threads: Mutex<ProcessThreads>,
    privilege: ProcessPrivilige,
    pub args: Vec<u8>,
    pub memory: Mutex<ProcessMemory>,
    pub service_messages: Mutex<ProcessMessages>,
}

#[derive(Default)]
pub struct ProcessThreads {
    // a reference to the process so that we can clone it for threads (it is weak to avoid a circular chain)
    proc_reference: Weak<Process>,
    thread_next_id: u64,
    pub threads: BTreeMap<ThreadID, Arc<Thread>>,
}

pub struct ProcessMemory {
    pub page_mapper: PageTable<'static, PageLvl4>,
    pub owned_pages: Vec<AllocatedPage>,
}

#[derive(Default)]
pub struct ProcessMessages {
    pub queue: VecDeque<Arc<(ServiceID, ServiceTrackingNumber, Box<[u8]>)>>,
    pub waiters: BTreeMap<(ServiceID, ServiceTrackingNumber), Vec<ThreadID>>,
}

impl Process {
    pub fn new(privilege: ProcessPrivilige, args: &[u8]) -> Arc<Self> {
        let pml4 = request_page().unwrap();
        let mut page_mapper = unsafe { PageTable::<PageLvl4>::from_page(*pml4) };
        let owned_pages = vec![pml4];

        unsafe {
            page_mapper.set_next_table(MemoryLoc::PhysMapOffset as u64, &mut *OFFSET_MAP.lock());
            page_mapper.set_next_table(MemoryLoc::KernelStart as u64, &mut *KERNEL_DATA_MAP.lock());
            page_mapper.set_next_table(MemoryLoc::KernelHeap as u64, &mut *KERNEL_HEAP_MAP.lock());
            page_mapper.set_next_table(MemoryLoc::PerCpuMem as u64, &mut *PER_CPU_MAP.lock());
            map_gop(&mut page_mapper);
            page_mapper
                .map_memory(
                    Page::<Size4KB>::new(0xfee000b0 & !0xFFF),
                    Page::new(0xfee000b0 & !0xFFF),
                )
                .unwrap()
                .ignore();
        }

        let s = Arc::new(Self {
            pid: generate_next_process_id(),
            privilege,
            args: args.to_vec(),
            memory: Mutex::new(ProcessMemory {
                page_mapper,
                owned_pages,
            }),
            threads: Default::default(),
            service_messages: Default::default(),
        });
        s.threads.lock().proc_reference = Arc::downgrade(&s);
        s
    }

    pub unsafe fn new_with_page(
        privilege: ProcessPrivilige,
        page_mapper: PageTable<'static, PageLvl4>,
        args: &[u8],
    ) -> Arc<Self> {
        let s = Arc::new(Self {
            pid: generate_next_process_id(),
            privilege,
            args: args.to_vec(),
            memory: Mutex::new(ProcessMemory {
                page_mapper,
                owned_pages: Vec::new(),
            }),
            threads: Default::default(),
            service_messages: Default::default(),
        });
        s.threads.lock().proc_reference = Arc::downgrade(&s);
        s
    }

    // A in place thread which data will overriden with the real thread on its context switch out.
    pub unsafe fn new_overide_thread(&self) -> Arc<Thread> {
        let mut threads = self.threads.lock();
        let tid = threads.get_next_id();

        let pushed_register_state = InterruptStackFrameValue {
            instruction_pointer: VirtAddr::new(0x8002),
            code_segment: 0,
            cpu_flags: 0,
            stack_pointer: VirtAddr::new(0),
            stack_segment: 0,
        };

        let register_state = Registers::default();

        let thread: Arc<Thread> = Arc::new(Thread {
            process: threads
                .proc_reference
                .upgrade()
                .expect("process should still be alive"),
            tid,
            context: Mutex::new(ThreadContext {
                register_state,
                pushed_register_state,
                current_message: Default::default(),
                schedule_status: ScheduleStatus::Scheduled,
            }),
        });

        threads.threads.insert(tid, thread.clone());
        thread
    }

    pub fn new_thread_direct(
        &self,
        entry_point: *const u64,
        register_state: Registers,
    ) -> Arc<Thread> {
        let mut threads = self.threads.lock();
        let tid = threads.get_next_id();

        // let stack_base = STACK_ADDR.fetch_add(0x1000_000, Ordering::Relaxed);
        let stack_base = STACK_ADDR + (STACK_SIZE + 0x1000) * tid.0;

        {
            let mut memory = self.memory.lock();
            for addr in (stack_base..(stack_base + STACK_SIZE - 1)).step_by(0x1000) {
                let page = request_page().unwrap();

                memory
                    .page_mapper
                    .map_memory(Page::new(addr), *page)
                    .unwrap()
                    .flush();

                memory.owned_pages.push(page);
            }
        }

        let pushed_register_state = InterruptStackFrameValue {
            instruction_pointer: VirtAddr::from_ptr(entry_point),
            code_segment: self.privilege.get_code_segment().0 as u64,
            cpu_flags: 0x202,
            stack_pointer: VirtAddr::new(stack_base + STACK_SIZE),
            stack_segment: self.privilege.get_data_segment().0 as u64,
        };

        let thread = Arc::new(Thread {
            process: threads.proc_reference.upgrade().unwrap(),
            tid,
            context: Mutex::new(ThreadContext {
                register_state,
                pushed_register_state,
                current_message: Default::default(),
                schedule_status: ScheduleStatus::Scheduled,
            }),
        });

        threads.threads.insert(tid, thread.clone());
        thread
    }

    pub fn new_thread(&self, entry_point: usize) -> Arc<Thread> {
        let register_state = Registers {
            rdi: entry_point,
            ..Default::default()
        };

        self.new_thread_direct(thread_bootstraper as *const u64, register_state)
    }
}

impl ProcessThreads {
    fn get_next_id(&mut self) -> ThreadID {
        let tid = ThreadID(self.thread_next_id);
        self.thread_next_id += 1;
        tid
    }
}

pub struct Thread {
    pub process: Arc<Process>,
    pub tid: ThreadID,
    pub context: Mutex<ThreadContext>,
}

pub struct ThreadContext {
    pub register_state: Registers,
    // Rest of the data inclusing rip & rsp
    pub pushed_register_state: InterruptStackFrameValue,
    // Used for storing current msg, so that the popdata can get the data
    pub current_message: Option<Arc<(ServiceID, ServiceTrackingNumber, Box<[u8]>)>>,
    // Is the thread scheduled or waiting
    pub schedule_status: ScheduleStatus,
}

#[derive(Debug, PartialEq, Eq)]
pub enum ScheduleStatus {
    Scheduled,
    WaitingOn(ServiceID),
}

impl ThreadContext {
    pub fn save(&mut self, stack_frame: &InterruptStackFrame, reg: &Registers) {
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
