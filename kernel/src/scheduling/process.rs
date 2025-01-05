use core::{
    cell::UnsafeCell,
    fmt::Debug,
    num::NonZeroUsize,
    sync::atomic::{AtomicU64, Ordering},
};

use alloc::{
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
    cpu_localstorage::CPULocalStorageRW,
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

use super::taskmanager::{ThreadSchedGlobalData, PROCESSES, SCHEDULER};

pub const STACK_ADDR: u64 = 0x100_000_000_000;
pub const KSTACK_ADDR: u64 = 0xffff_800_000_000_000;

pub const STACK_SIZE: u64 = 0x20000;
pub const KSTACK_SIZE: u64 = 0x10000;

pub const THREAD_TEMP_COUNT: usize = 8;

fn generate_next_process_id() -> ProcessID {
    static PID: AtomicU64 = AtomicU64::new(0);
    ProcessID(PID.fetch_add(1, Ordering::Relaxed))
}

#[derive(Debug, PartialEq, Eq, Clone, Copy)]
pub enum ProcessPrivilege {
    KERNEL,
    USER,
}

impl ProcessPrivilege {
    pub fn get_code_segment(&self) -> SegmentSelector {
        match self {
            ProcessPrivilege::KERNEL => gdt::KERNEL_CODE_SELECTOR,
            ProcessPrivilege::USER => gdt::USER_CODE_SELECTOR,
        }
    }

    pub fn get_data_segment(&self) -> SegmentSelector {
        match self {
            ProcessPrivilege::KERNEL => gdt::KERNEL_DATA_SELECTOR,
            ProcessPrivilege::USER => gdt::USER_DATA_SELECTOR,
        }
    }
}

pub struct Process {
    pub pid: ProcessID,
    pub threads: Spinlock<ProcessThreads>,
    pub privilege: ProcessPrivilege,
    pub args: Vec<u8>,
    pub memory: Spinlock<ProcessMemory>,
    pub references: Spinlock<ProcessReferences>,
    pub exit_status: Spinlock<ProcessExit>,
    pub signals: Spinlock<KObjectSignal>,
    pub name: &'static str,
}

#[derive(Default)]
pub struct ProcessThreads {
    thread_next_id: u64,
    pub threads: BTreeMap<ThreadID, Arc<Thread>>,
}

pub struct ProcessMemory {
    pub page_mapper: PageMapperManager,
    pub owned32_pages: Vec<AllocatedPage<GlobalPageAllocator>>,
}

impl ProcessMemory {
    pub fn new() -> Self {
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

        Self {
            page_mapper,
            owned32_pages: Vec::new(),
        }
    }
}

pub struct ProcessReferences {
    references: HashMap<KernelReferenceID, KernelValue>,
    next_id: usize,
}

impl ProcessReferences {
    pub fn new() -> Self {
        Self {
            references: HashMap::new(),
            next_id: 1,
        }
    }

    pub fn from_refs(refs_to_clone: &[KernelReferenceID]) -> Self {
        let mut refs = ProcessReferences::new();

        unsafe {
            let this = CPULocalStorageRW::get_current_task();
            let mut this_refs = this.process().references.lock();
            for r in refs_to_clone {
                refs.add_value(
                    this_refs
                        .references()
                        .get(r)
                        .expect("the references should belong to the calling process")
                        .clone(),
                );
            }
        }
        refs
    }

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

impl Default for ProcessReferences {
    fn default() -> Self {
        Self::new()
    }
}

impl Process {
    pub fn new(
        privilege: ProcessPrivilege,
        memory: ProcessMemory,
        references: ProcessReferences,
        args: Vec<u8>,
        name: &'static str,
    ) -> Arc<Self> {
        Arc::new(Self {
            pid: generate_next_process_id(),
            privilege,
            args: args,
            memory: Spinlock::new(memory),
            threads: Default::default(),
            references: Spinlock::new(references),
            exit_status: Spinlock::new(ProcessExit::NotExitedYet),
            signals: Default::default(),
            name,
        })
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
            t.1.sched().lock().killed = true;
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
    unsafe extern "C" fn after_start_cleanup() {
        let thread = CPULocalStorageRW::get_current_task();
        // We must unlock the mutex after return from scheduler
        thread.sched().force_unlock();
    }

    unsafe {
        core::arch::naked_asm!(
            "mov cl, 2",
            "mov gs:0x9, cl", // set cpu context
            "call {}",
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
            sym after_start_cleanup
        );
    }
}

pub struct ProcessBuilder {
    name: &'static str,
    privilege: ProcessPrivilege,
    references: Option<ProcessReferences>,
    vm: ProcessMemory,
    entry_point: *const u64,
    arg: usize,
    args: Vec<u8>,
}

impl ProcessBuilder {
    pub const fn new(vm: ProcessMemory, entry_point: *const u64, arg: usize) -> Self {
        Self {
            name: "",
            privilege: ProcessPrivilege::USER,
            args: Vec::new(),
            vm,
            entry_point,
            references: None,
            arg,
        }
    }

    pub fn name(mut self, name: &'static str) -> Self {
        self.name = name;
        self
    }

    pub fn privilege(mut self, privilege: ProcessPrivilege) -> Self {
        self.privilege = privilege;
        self
    }

    pub fn args(mut self, args: Vec<u8>) -> Self {
        self.args = args;
        self
    }

    pub fn references(mut self, refs: ProcessReferences) -> Self {
        self.references = Some(refs);
        self
    }

    pub fn build(self) -> Arc<Process> {
        let proc = Process::new(
            self.privilege,
            self.vm,
            self.references.unwrap_or_default(),
            self.args,
            self.name,
        );
        PROCESSES.lock().insert(proc.pid, proc.clone());
        let thread = Thread::new(proc.clone(), self.entry_point, self.arg).unwrap();
        SCHEDULER.lock().queue_thread(thread);
        proc
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

pub struct Thread {
    weak_self: Weak<Thread>,
    process: Arc<Process>,
    tid: ThreadID,

    sched_global: ThreadSchedGlobal,
    sched: Spinlock<ThreadSched>,
}

impl Debug for Thread {
    fn fmt(&self, f: &mut core::fmt::Formatter<'_>) -> core::fmt::Result {
        f.debug_struct("Thread")
            .field("pid", &self.process.pid)
            .field("tid", &self.tid)
            .field("name", &self.process.name)
            .finish()
    }
}

impl Thread {
    pub fn new(process: Arc<Process>, entry_point: *const u64, arg: usize) -> Option<Arc<Thread>> {
        let mut threads = process.threads.lock();
        let tid = threads.get_next_id();

        // let stack_base = STACK_ADDR.fetch_add(0x1000_000, Ordering::Relaxed);
        let stack_base = STACK_ADDR + (STACK_SIZE + 0x1000) * tid.0;

        let stack = match process.privilege {
            ProcessPrivilege::KERNEL => PageMapping::new_lazy_filled(STACK_SIZE as usize),
            _ => PageMapping::new_lazy(STACK_SIZE as usize),
        };

        process
            .memory
            .lock()
            .page_mapper
            .insert_mapping_at_set(stack_base as usize, stack, MemoryMappingFlags::all())
            .unwrap();

        let kstack_base = KSTACK_ADDR + (KSTACK_SIZE + 0x1000) * tid.0;
        let kstack_top = (kstack_base + KSTACK_SIZE) as usize;
        let stack = PageMapping::new_lazy_filled(KSTACK_SIZE as usize);
        let kstack_ptr_for_start = stack.base_top_stack();
        let kstack_base_virt = virt_addr_for_phys(kstack_ptr_for_start as u64) as usize;

        process
            .memory
            .lock()
            .page_mapper
            .insert_mapping_at_set(kstack_base as usize, stack, MemoryMappingFlags::WRITEABLE)
            .unwrap();

        let interrupt_frame = InterruptStackFrameValue {
            instruction_pointer: VirtAddr::from_ptr(entry_point),
            code_segment: process.privilege.get_code_segment().0 as u64,
            cpu_flags: 0x202,
            stack_pointer: VirtAddr::new(stack_base + STACK_SIZE),
            stack_segment: process.privilege.get_data_segment().0 as u64,
        };

        unsafe { *(kstack_base_virt as *mut usize) = arg };
        unsafe { *((kstack_base_virt + 8) as *mut InterruptStackFrameValue) = interrupt_frame }
        let thread = Arc::new_cyclic(|this| Thread {
            weak_self: this.clone(),
            process: process.clone(),
            tid,
            sched_global: ThreadSchedGlobal::new(),
            sched: Spinlock::new(ThreadSched {
                state: ThreadState::Runnable,
                task_state: Some(SavedTaskState {
                    sp: kstack_top - 0x1000,
                    ip: start_new_task as usize,
                }),
                kstack_top: VirtAddr::from_ptr(kstack_top as *const ()),
                in_syscall: false,
                killed: false,
                cr3_page: unsafe {
                    process
                        .memory
                        .lock()
                        .page_mapper
                        .get_mapper_mut()
                        .get_physical_address() as u64
                },
            }),
        });

        threads.threads.insert(tid, thread.clone());

        return Some(thread);
    }

    pub fn thread(&self) -> Arc<Thread> {
        self.weak_self.upgrade().unwrap()
    }

    pub fn process(&self) -> &Arc<Process> {
        &self.process
    }

    pub fn tid(&self) -> ThreadID {
        self.tid
    }

    /// SAFTEY: Must hold the global sched lock
    pub unsafe fn sched_global(&self) -> &mut ThreadSchedGlobalData {
        &mut *self.sched_global.0.get()
    }

    pub fn sched(&self) -> &Spinlock<ThreadSched> {
        &self.sched
    }

    pub fn wake(&self) {
        let mut s = self.sched.lock();
        match s.state {
            ThreadState::Zombie | ThreadState::Runnable => (),
            ThreadState::Sleeping => {
                s.state = ThreadState::Runnable;
                drop(s);
                SCHEDULER
                    .lock()
                    .queue_thread(self.weak_self.upgrade().unwrap());
            }
        }
    }
}

/// Data used for the scheduler blocked behind the global lock
pub struct ThreadSchedGlobal(UnsafeCell<ThreadSchedGlobalData>);

/// The sched global is safe, we must hold lock to access it
unsafe impl Send for ThreadSchedGlobal {}
unsafe impl Sync for ThreadSchedGlobal {}

impl ThreadSchedGlobal {
    pub const fn new() -> Self {
        Self(UnsafeCell::new(ThreadSchedGlobalData::new()))
    }
}

pub struct ThreadSched {
    pub state: ThreadState,
    pub task_state: Option<SavedTaskState>,
    pub kstack_top: VirtAddr,
    pub in_syscall: bool,
    pub killed: bool,
    pub cr3_page: u64,
}

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum ThreadState {
    Zombie,
    Runnable,
    Sleeping,
}

impl ThreadSched {
    pub fn save(&mut self, state: SavedTaskState) {
        assert!(self.task_state.is_none());
        self.task_state = Some(state);
    }
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

impl Drop for Thread {
    fn drop(&mut self) {
        let stack_base = STACK_ADDR + (STACK_SIZE + 0x1000) * self.tid.0;

        unsafe {
            self.process
                .memory
                .lock()
                .page_mapper
                .free_mapping(stack_base as usize..(stack_base + STACK_SIZE) as usize)
                .unwrap();
        }
    }
}
