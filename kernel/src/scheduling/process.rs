use core::{
    fmt::Debug,
    sync::atomic::{AtomicU64, Ordering},
};

use alloc::{
    boxed::Box,
    collections::{BTreeMap, VecDeque},
    sync::{Arc, Weak},
    vec::Vec,
};
use hashbrown::HashMap;
use kernel_userspace::{
    ids::{ProcessID, ServiceID, ThreadID},
    message::MessageId,
    service::ServiceMessageDesc,
};
use spin::{Lazy, Mutex};
use x86_64::{
    structures::{
        gdt::SegmentSelector,
        idt::{InterruptStackFrame, InterruptStackFrameValue},
    },
    VirtAddr,
};

use crate::{
    assembly::registers::{Registers, SavedThreadState},
    gdt,
    message::{KMessage, KMessageProcRefcount},
    paging::{
        offset_map::get_gop_range,
        page_allocator::Allocated32Page,
        page_mapper::{PageMapperManager, PageMapping},
        MemoryLoc, MemoryMappingFlags, KERNEL_DATA_MAP, KERNEL_HEAP_MAP, OFFSET_MAP, PER_CPU_MAP,
    },
    BOOT_INFO,
};

pub const STACK_ADDR: u64 = 0x100_000_000_000;

pub const STACK_SIZE: u64 = 0x10000;

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
    // a reference to the process so that we can clone it for threads (it is weak to avoid a circular chain)
    this: Weak<Process>,
    pub pid: ProcessID,
    pub threads: Mutex<ProcessThreads>,
    pub privilege: ProcessPrivilige,
    pub args: Vec<u8>,
    pub memory: Mutex<ProcessMemory>,
    pub service_messages: Mutex<ProcessMessages>,
}

#[derive(Default)]
pub struct ProcessThreads {
    thread_next_id: u64,
    pub threads: BTreeMap<ThreadID, Arc<ThreadHandle>>,
}

pub struct ProcessMemory {
    pub page_mapper: PageMapperManager<'static>,
    pub owned32_pages: Vec<Allocated32Page>,
}

#[derive(Default)]
pub struct ProcessMessages {
    pub messages: HashMap<MessageId, KMessageProcRefcount>,
    pub queue: HashMap<ServiceID, ServiceQueue>,
}

#[derive(Default)]
pub struct ServiceQueue {
    pub message_queue: VecDeque<Arc<(ServiceMessageDesc, Arc<KMessage>)>>,
    pub wakers: Vec<Box<Thread>>,
}

impl Process {
    pub fn new(privilege: ProcessPrivilige, args: &[u8]) -> Arc<Self> {
        let mut page_mapper = PageMapperManager::new();

        unsafe {
            let m = page_mapper.get_mapper_mut();
            m.set_next_table(MemoryLoc::PhysMapOffset as u64, &mut *OFFSET_MAP.lock());
            m.set_next_table(MemoryLoc::KernelStart as u64, &mut *KERNEL_DATA_MAP.lock());
            m.set_next_table(MemoryLoc::KernelHeap as u64, &mut *KERNEL_HEAP_MAP.lock());
            m.set_next_table(MemoryLoc::PerCpuMem as u64, &mut *PER_CPU_MAP.lock());

            let gop = get_gop_range(&(*BOOT_INFO).gop);
            page_mapper
                .insert_mapping_at_set(gop.0, gop.1, MemoryMappingFlags::WRITEABLE)
                .unwrap();

            static APIC_LOCATION: Lazy<Arc<PageMapping>> =
                Lazy::new(|| unsafe { PageMapping::new_mmap(0xfee00000, 0x1000) });

            page_mapper
                .insert_mapping_at_set(
                    0xfee00000,
                    APIC_LOCATION.clone(),
                    MemoryMappingFlags::WRITEABLE,
                )
                .unwrap();
        }

        Arc::new_cyclic(|this| Self {
            this: this.clone(),
            pid: generate_next_process_id(),
            privilege,
            args: args.to_vec(),
            memory: Mutex::new(ProcessMemory {
                page_mapper,
                owned32_pages: Default::default(),
            }),
            threads: Default::default(),
            service_messages: Default::default(),
        })
    }

    pub fn new_thread_direct(
        &self,
        entry_point: *const u64,
        register_state: Registers,
    ) -> Box<Thread> {
        let mut threads = self.threads.lock();
        let tid = threads.get_next_id();

        // let stack_base = STACK_ADDR.fetch_add(0x1000_000, Ordering::Relaxed);
        let stack_base = STACK_ADDR + (STACK_SIZE + 0x1000) * tid.0;

        let stack = match self.privilege {
            ProcessPrivilige::KERNEL => PageMapping::new_lazy_filled(STACK_SIZE as usize),
            ProcessPrivilige::USER => PageMapping::new_lazy(STACK_SIZE as usize),
        };

        self.memory
            .lock()
            .page_mapper
            .insert_mapping_at_set(stack_base as usize, stack, MemoryMappingFlags::all())
            .unwrap();

        let interrupt_frame = InterruptStackFrameValue {
            instruction_pointer: VirtAddr::from_ptr(entry_point),
            code_segment: self.privilege.get_code_segment().0 as u64,
            cpu_flags: 0x202,
            stack_pointer: VirtAddr::new(stack_base + STACK_SIZE),
            stack_segment: self.privilege.get_data_segment().0 as u64,
        };

        let handle = Arc::new(ThreadHandle {
            process: self.this.upgrade().unwrap(),
            tid,
        });

        threads.threads.insert(tid, handle.clone());
        Box::new(Thread {
            handle,
            state: SavedThreadState {
                register_state,
                interrupt_frame,
            },
        })
    }

    pub fn new_thread(&self, entry_point: *const u64, arg: usize) -> Box<Thread> {
        let register_state = Registers {
            rdi: arg,
            ..Default::default()
        };

        self.new_thread_direct(entry_point, register_state)
    }
}

impl ProcessThreads {
    fn get_next_id(&mut self) -> ThreadID {
        let tid = ThreadID(self.thread_next_id);
        self.thread_next_id += 1;
        tid
    }
}

pub struct ThreadHandle {
    process: Arc<Process>,
    tid: ThreadID,
}

impl ThreadHandle {
    pub fn tid(&self) -> ThreadID {
        self.tid
    }

    pub fn process(&self) -> &Arc<Process> {
        &self.process
    }
}

pub struct Thread {
    handle: Arc<ThreadHandle>,
    state: SavedThreadState,
}

impl Thread {
    pub fn handle(&self) -> &Arc<ThreadHandle> {
        &self.handle
    }

    pub fn process(&self) -> &Arc<Process> {
        &self.handle.process
    }

    pub fn state(&self) -> &SavedThreadState {
        &self.state
    }

    pub unsafe fn save_state(&mut self, stack_frame: &InterruptStackFrame, reg: &Registers) {
        self.state = SavedThreadState::new(stack_frame, reg)
    }

    pub unsafe fn restore_state(&self, stack_frame: &mut InterruptStackFrame, reg: &mut Registers) {
        stack_frame
            .as_mut()
            .extract_inner()
            .clone_from(&self.state.interrupt_frame);
        reg.clone_from(&self.state.register_state);
    }
}

impl Debug for Thread {
    fn fmt(&self, f: &mut core::fmt::Formatter<'_>) -> core::fmt::Result {
        f.debug_struct("Thread")
            .field("tid", &self.handle.tid)
            .field("state", &self.state)
            .finish()
    }
}

impl Drop for Thread {
    fn drop(&mut self) {
        let stack_base = STACK_ADDR + (STACK_SIZE + 0x1000) * self.handle.tid.0;

        unsafe {
            self.handle
                .process
                .memory
                .lock()
                .page_mapper
                .free_mapping(stack_base as usize..(stack_base + STACK_SIZE) as usize)
                .unwrap();
        }
    }
}
