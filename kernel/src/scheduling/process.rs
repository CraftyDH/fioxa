use core::{
    fmt::Debug,
    num::NonZeroUsize,
    sync::atomic::{AtomicBool, AtomicU64, Ordering},
};

use alloc::{
    boxed::Box,
    collections::BTreeMap,
    sync::{Arc, Weak},
    vec::Vec,
};
use conquer_once::spin::Lazy;
use hashbrown::HashMap;
use kernel_userspace::{
    ids::{ProcessID, ThreadID},
    object::{KernelObjectType, KernelReferenceID, ObjectSignal},
    process::ProcessExit,
};
use x86_64::{
    structures::{gdt::SegmentSelector, idt::InterruptStackFrameValue},
    VirtAddr,
};

use crate::{
    assembly::registers::SavedTaskState,
    channel::KChannelHandle,
    gdt,
    interrupts::KInterruptHandle,
    message::KMessage,
    mutex::Spinlock,
    object::{KObject, KObjectSignal},
    paging::{
        page_allocator::global_allocator,
        page_mapper::{PageMapperManager, PageMapping},
        virt_addr_for_phys, AllocatedPage, GlobalPageAllocator, MemoryMappingFlags,
    },
    port::KPort,
    time::HPET,
};

use super::taskmanager::{push_task_queue, PROCESSES};

pub const STACK_ADDR: u64 = 0x100_000_000_000;
pub const KSTACK_ADDR: u64 = 0xffff_800_000_000_000;

pub const STACK_SIZE: u64 = 0x20000;
pub const KSTACK_SIZE: u64 = 0x10000;

pub const THREAD_TEMP_COUNT: usize = 8;

fn generate_next_process_id() -> ProcessID {
    static PID: AtomicU64 = AtomicU64::new(0);
    ProcessID(PID.fetch_add(1, Ordering::Relaxed))
}

#[derive(Debug, PartialEq, Eq)]
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
    pub threads: Spinlock<ProcessThreads>,
    pub privilege: ProcessPrivilige,
    pub args: Vec<u8>,
    pub memory: Spinlock<ProcessMemory>,
    pub cr3_page: u64,
    pub references: Spinlock<ProcessReferences>,
    pub exit_status: Spinlock<ProcessExit>,
    pub signals: Spinlock<KObjectSignal>,
}

#[derive(Default)]
pub struct ProcessThreads {
    thread_next_id: u64,
    pub threads: BTreeMap<ThreadID, Arc<ThreadHandle>>,
}

pub struct ProcessMemory {
    pub page_mapper: PageMapperManager,
    pub owned32_pages: Vec<AllocatedPage<GlobalPageAllocator>>,
}

pub struct ProcessReferences {
    references: HashMap<KernelReferenceID, KernelValue>,
    next_id: usize,
}

impl ProcessReferences {
    pub fn references(&mut self) -> &mut HashMap<KernelReferenceID, KernelValue> {
        &mut self.references
    }

    pub fn add_value(&mut self, value: KernelValue) -> KernelReferenceID {
        let id = KernelReferenceID(NonZeroUsize::new(self.next_id).unwrap());
        self.next_id += 1;
        assert!(self.references.insert(id, value).is_none());
        id
    }
}

impl Process {
    pub fn new(privilege: ProcessPrivilige, args: &[u8]) -> Arc<Self> {
        let mut page_mapper = PageMapperManager::new(global_allocator());

        static APIC_LOCATION: Lazy<Arc<PageMapping>> =
            Lazy::new(|| unsafe { PageMapping::new_mmap(0xfee00000, 0x1000) });

        static HPET_LOCATION: Lazy<(usize, Arc<PageMapping>)> = Lazy::new(|| unsafe {
            let val = HPET.get().unwrap().info.base_address;
            (val, PageMapping::new_mmap(val, 0x1000))
        });

        page_mapper
            .insert_mapping_at_set(
                0xfee00000,
                APIC_LOCATION.clone(),
                MemoryMappingFlags::WRITEABLE,
            )
            .unwrap();

        // Slightly scary, but only init will not and it should map it itself
        if HPET.is_initialized() {
            page_mapper
                .insert_mapping_at_set(
                    HPET_LOCATION.0,
                    HPET_LOCATION.1.clone(),
                    MemoryMappingFlags::WRITEABLE,
                )
                .unwrap();
        }

        Arc::new_cyclic(|this| Self {
            this: this.clone(),
            pid: generate_next_process_id(),
            privilege,
            args: args.to_vec(),
            cr3_page: unsafe { page_mapper.get_mapper_mut().get_physical_address() as u64 },
            memory: Spinlock::new(ProcessMemory {
                page_mapper,
                owned32_pages: Default::default(),
            }),
            threads: Default::default(),
            references: Spinlock::new(ProcessReferences {
                references: Default::default(),
                next_id: 1,
            }),
            exit_status: Spinlock::new(ProcessExit::NotExitedYet),
            signals: Default::default(),
        })
    }

    pub fn new_thread(&self, entry_point: *const u64, arg: usize) -> Option<Box<Thread>> {
        let mut threads = self.threads.lock();
        let tid = threads.get_next_id();

        // let stack_base = STACK_ADDR.fetch_add(0x1000_000, Ordering::Relaxed);
        let stack_base = STACK_ADDR + (STACK_SIZE + 0x1000) * tid.0;

        let stack = match self.privilege {
            ProcessPrivilige::KERNEL => PageMapping::new_lazy_filled(STACK_SIZE as usize),
            _ => PageMapping::new_lazy(STACK_SIZE as usize),
        };

        self.memory
            .lock()
            .page_mapper
            .insert_mapping_at_set(stack_base as usize, stack, MemoryMappingFlags::all())
            .unwrap();

        let kstack_base = KSTACK_ADDR + (KSTACK_SIZE + 0x1000) * tid.0;
        let kstack_top = (kstack_base + KSTACK_SIZE) as usize;
        let stack = PageMapping::new_lazy_filled(KSTACK_SIZE as usize);
        let kstack_ptr_for_start = stack.base_top_stack();
        let kstack_base_virt = virt_addr_for_phys(kstack_ptr_for_start as u64) as usize;

        self.memory
            .lock()
            .page_mapper
            .insert_mapping_at_set(kstack_base as usize, stack, MemoryMappingFlags::WRITEABLE)
            .unwrap();

        let interrupt_frame = InterruptStackFrameValue {
            instruction_pointer: VirtAddr::from_ptr(entry_point),
            code_segment: self.privilege.get_code_segment().0 as u64,
            cpu_flags: 0x202,
            stack_pointer: VirtAddr::new(stack_base + STACK_SIZE),
            stack_segment: self.privilege.get_data_segment().0 as u64,
        };

        unsafe { *(kstack_base_virt as *mut usize) = arg };
        unsafe { *((kstack_base_virt + 8) as *mut InterruptStackFrameValue) = interrupt_frame }
        let handle = Arc::new(ThreadHandle {
            process: self.this.upgrade().unwrap(),
            tid,
            thread: Spinlock::new(None),
            kill_signal: AtomicBool::new(false),
        });

        let status = self.exit_status.lock();

        if let ProcessExit::Exited = *status {
            return None;
        }

        threads.threads.insert(tid, handle.clone());
        Some(Box::new(Thread {
            handle,
            kstack_top: VirtAddr::from_ptr(kstack_top as *const ()),
            state: Some(SavedTaskState {
                sp: kstack_top - 0x1000,
                ip: start_new_task as usize,
            }),
            linked_next: None,
            in_syscall: false,
        }))
    }

    pub fn add_value(&self, value: KernelValue) -> KernelReferenceID {
        self.references.lock().add_value(value)
    }

    pub fn get_value(&self, id: KernelReferenceID) -> Option<KernelValue> {
        self.references.lock().references.get(&id).cloned()
    }

    pub fn kill_threads(&self) {
        let threads = self.threads.lock();
        for t in &threads.threads {
            t.1.kill_signal.store(true, Ordering::Relaxed);
        }
        if threads.threads.is_empty() {
            drop(threads);
            *self.exit_status.lock() = ProcessExit::Exited;
            self.signals
                .lock()
                .set_signal(ObjectSignal::PROCESS_EXITED, true);
            PROCESSES.lock().remove(&self.pid);
        }
    }
}

impl KObject for Process {
    fn signals<T>(&self, f: impl FnOnce(&mut KObjectSignal) -> T) -> T {
        f(&mut self.signals.lock())
    }
}

#[naked]
pub extern "C" fn start_new_task(arg: usize) {
    unsafe {
        core::arch::naked_asm!(
            "mov cl, 2",
            "mov gs:0x9, cl", // set cpu context
            "pop rdi",
            // Zero registers (except rdi which has arg)
            "xor r15d, r15d",
            "xor r14d, r14d",
            "xor r13d, r13d",
            "xor r12d, r12d",
            "xor r11d, r11d",
            "xor r10d, r10d",
            "xor r9d,  r9d",
            "xor r8d,  r8d",
            "xor esi,  esi",
            "xor edx,  edx",
            "xor ecx,  ecx",
            "xor ebx,  ebx",
            "xor eax,  eax",
            "xor ebp,  ebp",
            // start
            "iretq",
        );
    }
}

// Returns null if unknown process
pub fn share_kernel_value(value: KernelValue, proc: ProcessID) -> Option<KernelReferenceID> {
    PROCESSES.lock().get(&proc).map(|p| p.add_value(value))
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
    pub thread: Spinlock<Option<Box<Thread>>>,
    pub kill_signal: AtomicBool,
}

impl ThreadHandle {
    pub fn tid(&self) -> ThreadID {
        self.tid
    }

    pub fn process(&self) -> &Arc<Process> {
        &self.process
    }

    pub fn wake_up(&self) {
        if let Some(thread) = self.thread.lock().take() {
            push_task_queue(thread)
        }
    }
}

pub struct LinkedThreadList {
    head: Option<Box<Thread>>,
    tail: Option<*mut Thread>,
}

unsafe impl Send for LinkedThreadList {}

impl LinkedThreadList {
    pub const fn new() -> Self {
        Self {
            head: None,
            tail: None,
        }
    }

    pub fn push(&mut self, mut thread: Box<Thread>) {
        let next_addr = Some(thread.as_mut() as *mut Thread);
        let next_tail = Some(thread);
        match self.tail {
            Some(addr) => unsafe {
                let tail = &mut *addr;
                assert!(
                    tail.linked_next.is_none(),
                    "the tail shouldn't have any linked elements"
                );
                tail.linked_next = next_tail;
            },
            None => {
                assert!(self.head.is_none(), "if tail is none, head should be none");
                self.head = next_tail;
            }
        }
        self.tail = next_addr;
    }

    pub fn pop(&mut self) -> Option<Box<Thread>> {
        let mut el = self.head.take()?;
        match el.linked_next.take() {
            Some(nxt) => self.head = Some(nxt),
            None => self.tail = None,
        }
        Some(el)
    }

    /// Takes all threads from other and places them in here.
    pub fn append(&mut self, other: &mut LinkedThreadList) {
        let Some(nxt) = other.head.take() else { return };
        match self.tail {
            Some(addr) => unsafe {
                let tail = &mut *addr;
                assert!(
                    tail.linked_next.is_none(),
                    "the tail shouldn't have any linked elements"
                );
                tail.linked_next = Some(nxt);
            },
            None => {
                assert!(self.head.is_none(), "if tail is none, head should be none");
                self.head = Some(nxt)
            }
        }
        self.tail = other.tail.take();
    }
}

impl Default for LinkedThreadList {
    fn default() -> Self {
        Self::new()
    }
}

pub struct Thread {
    handle: Arc<ThreadHandle>,
    pub state: Option<SavedTaskState>,
    pub kstack_top: VirtAddr,
    linked_next: Option<Box<Thread>>,
    // if true do not kill as it might hold resources
    pub in_syscall: bool,
}

#[derive(Clone)]
pub enum KernelValue {
    Message(Arc<KMessage>),
    Process(Arc<Process>),
    Channel(Arc<KChannelHandle>),
    Port(Arc<KPort>),
    Interrupt(Arc<KInterruptHandle>),
}

impl Debug for KernelValue {
    fn fmt(&self, f: &mut core::fmt::Formatter<'_>) -> core::fmt::Result {
        match self {
            Self::Message(_) => f.debug_tuple("KernelValue::Message").finish(),
            Self::Process(_) => f.debug_tuple("KernelValue::Process").finish(),
            Self::Channel(_) => f.debug_tuple("KernelValue::Channel").finish(),
            Self::Port(_) => f.debug_tuple("KernelValue::Port").finish(),
            Self::Interrupt(_) => f.debug_tuple("KernelValue::Interrupt").finish(),
        }
    }
}

impl KernelValue {
    pub const fn object_type(&self) -> KernelObjectType {
        match self {
            KernelValue::Message(_) => KernelObjectType::Message,
            KernelValue::Process(_) => KernelObjectType::Process,
            KernelValue::Channel(_) => KernelObjectType::Channel,
            KernelValue::Port(_) => KernelObjectType::Port,
            KernelValue::Interrupt(_) => KernelObjectType::Interrupt,
        }
    }
}

impl Into<KernelValue> for Arc<KMessage> {
    fn into(self) -> KernelValue {
        KernelValue::Message(self)
    }
}

impl Into<KernelValue> for Arc<Process> {
    fn into(self) -> KernelValue {
        KernelValue::Process(self)
    }
}

impl Into<KernelValue> for Arc<KChannelHandle> {
    fn into(self) -> KernelValue {
        KernelValue::Channel(self)
    }
}

impl Into<KernelValue> for Arc<KPort> {
    fn into(self) -> KernelValue {
        KernelValue::Port(self)
    }
}

impl Into<KernelValue> for Arc<KInterruptHandle> {
    fn into(self) -> KernelValue {
        KernelValue::Interrupt(self)
    }
}

impl Thread {
    pub fn handle(&self) -> &Arc<ThreadHandle> {
        &self.handle
    }

    pub fn process(&self) -> &Arc<Process> {
        &self.handle.process
    }

    pub fn save(&mut self, state: SavedTaskState) {
        assert!(self.state.is_none());
        self.state = Some(state);
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
