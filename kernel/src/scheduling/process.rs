use core::{
    cell::UnsafeCell,
    fmt::Debug,
    num::NonZeroUsize,
    sync::atomic::{AtomicU64, Ordering},
};

use alloc::{
    boxed::Box,
    collections::BTreeMap,
    sync::{Arc, Weak},
    vec::Vec,
};
use kernel_sys::types::{Hid, KernelObjectType, Pid, RawValue, Tid, VMMapFlags, VMOAnonymousFlags};
use slab::Slab;
use spin::Lazy;
use x86_64::{
    VirtAddr,
    registers::rflags::RFlags,
    structures::{gdt::SegmentSelector, idt::InterruptStackFrameValue},
};

use crate::{
    assembly::registers::SavedTaskState,
    channel::KChannelHandle,
    cpu_localstorage::{CPULocalStorage, CPULocalStorageRW},
    gdt,
    interrupts::KInterruptHandle,
    message::KMessage,
    mutex::Spinlock,
    object::{KObject, KObjectSignal},
    paging::{
        AllocatedPage, GlobalPageAllocator, KERNEL_STACKS_MAP, MemoryLoc,
        page_allocator::global_allocator,
        page_table::{EntryMut, MaybeOwned},
    },
    port::KPort,
    time::HPET,
    vm::{VMO, VirtualMemoryRegion},
};

use super::taskmanager::{PROCESSES, SCHEDULER, ThreadSchedGlobalData};

pub const STACK_ADDR: u64 = 0x100_000_000_000;

pub const STACK_SIZE: u64 = 0x20000;
pub const KSTACK_SIZE: u64 = 0x10000;

pub const THREAD_TEMP_COUNT: usize = 8;

fn generate_next_process_id() -> Pid {
    static PID: AtomicU64 = AtomicU64::new(1);
    Pid::from_raw(PID.fetch_add(1, Ordering::Relaxed)).unwrap()
}

fn generate_next_kstack_id() -> u64 {
    static STACK: AtomicU64 = AtomicU64::new(0);
    STACK.fetch_add(1, Ordering::Relaxed)
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
    pub pid: Pid,
    pub threads: Spinlock<ProcessThreads>,
    pub privilege: ProcessPrivilege,
    pub args: Vec<u8>,
    pub memory: Spinlock<ProcessMemory>,
    pub references: Spinlock<ProcessReferences>,
    pub exit_status: Spinlock<Option<usize>>,
    pub signals: Spinlock<KObjectSignal>,
    pub name: &'static str,
}

#[derive(Default)]
pub struct ProcessThreads {
    thread_next_id: u64,
    pub threads: BTreeMap<Tid, Arc<Thread>>,
}

pub struct ProcessMemory {
    pub region: VirtualMemoryRegion,
}

impl ProcessMemory {
    pub fn new() -> Option<Self> {
        let mut region = VirtualMemoryRegion::new(global_allocator())?;

        static HPET_LOCATION: Lazy<(usize, Arc<Spinlock<VMO>>)> = Lazy::new(|| unsafe {
            let val = HPET.get().unwrap().info.base_address;
            (val, Arc::new(Spinlock::new(VMO::new_mmap(val, 0x1000))))
        });

        // Slightly scary, but only init will not and it should map it itself
        if HPET.is_completed() {
            region
                .map_vmo(
                    HPET_LOCATION.1.clone(),
                    VMMapFlags::WRITEABLE,
                    Some(HPET_LOCATION.0),
                )
                .unwrap();
        }

        Some(Self { region })
    }
}

#[derive(Debug, Clone)]
pub struct ProcessReferences(Slab<KernelValue>);

impl ProcessReferences {
    pub const fn new() -> Self {
        Self(Slab::new())
    }

    pub fn from_refs(refs_to_clone: &[Hid]) -> Self {
        let mut refs = ProcessReferences::new();

        unsafe {
            let this = CPULocalStorageRW::get_current_task();
            let this_refs = this.process().references.lock();
            for r in refs_to_clone {
                refs.insert(
                    this_refs
                        .get(*r)
                        .expect("the references should belong to the calling process")
                        .clone(),
                );
            }
        }
        refs
    }

    pub fn get(&self, hid: Hid) -> Option<&KernelValue> {
        self.0.get(hid.0.get() - 1)
    }

    pub fn get_mut(&mut self, hid: Hid) -> Option<&mut KernelValue> {
        self.0.get_mut(hid.0.get() - 1)
    }

    pub fn remove(&mut self, hid: Hid) -> Option<KernelValue> {
        self.0.try_remove(hid.0.get() - 1)
    }

    pub fn insert(&mut self, value: KernelValue) -> Hid {
        Hid(NonZeroUsize::new(self.0.insert(value) + 1).unwrap())
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
            args,
            memory: Spinlock::new(memory),
            threads: Default::default(),
            references: Spinlock::new(references),
            exit_status: Spinlock::new(None),
            signals: Default::default(),
            name,
        })
    }

    pub fn add_value(&self, value: KernelValue) -> Hid {
        self.references.lock().insert(value)
    }

    pub fn get_value(&self, id: Hid) -> Option<KernelValue> {
        self.references.lock().get(id).cloned()
    }
}

impl KObject for Process {
    fn signals<T>(&self, f: impl FnOnce(&mut KObjectSignal) -> T) -> T {
        f(&mut self.signals.lock())
    }
}

#[unsafe(naked)]
pub unsafe extern "C" fn start_new_task(arg: usize) {
    unsafe extern "C" fn after_start_cleanup() {
        unsafe {
            let thread = CPULocalStorageRW::get_current_task();
            // We must unlock the mutex after return from scheduler
            thread.sched().force_unlock();
        }
    }

    core::arch::naked_asm!(
        "mov cl, 2",
        "mov gs:{ctx}, cl", // set cpu context
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
        sym after_start_cleanup,
        ctx = const core::mem::offset_of!(CPULocalStorage, current_context),
    );
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
pub fn share_kernel_value(value: KernelValue, proc: Pid) -> Option<Hid> {
    PROCESSES.lock().get(&proc).map(|p| p.add_value(value))
}

impl ProcessThreads {
    fn get_next_id(&mut self) -> Tid {
        self.thread_next_id += 1;
        Tid::from_raw(self.thread_next_id).unwrap()
    }
}

pub struct Thread {
    weak_self: Weak<Thread>,
    process: Arc<Process>,
    tid: Tid,
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
        let stack_base = STACK_ADDR + (STACK_SIZE * 2) * tid.into_raw();

        let stack_flags = match process.privilege {
            ProcessPrivilege::KERNEL => VMOAnonymousFlags::PINNED,
            _ => VMOAnonymousFlags::empty(),
        };

        let stack = Arc::new(Spinlock::new(VMO::new_anonymous(
            STACK_SIZE as usize,
            stack_flags,
        )));

        process
            .memory
            .lock()
            .region
            .map_vmo(
                stack,
                VMMapFlags::WRITEABLE | VMMapFlags::USERSPACE,
                Some(stack_base as usize),
            )
            .unwrap();

        let kstack_id = generate_next_kstack_id();

        let alloc = global_allocator();

        let kbase = MemoryLoc::KernelStacks as u64 + KSTACK_SIZE * 2 * kstack_id;
        let mut lvl3 = KERNEL_STACKS_MAP.lock();
        let flags = VMMapFlags::WRITEABLE;
        for page in (kbase..kbase + KSTACK_SIZE).step_by(0x1000) {
            let frame = AllocatedPage::new(alloc).unwrap();
            let addr = page as usize;
            let lvl2 = lvl3.as_mut().get_mut(addr).try_table(flags, alloc).unwrap();
            let lvl1 = lvl2.get_mut(addr).try_table(flags, alloc).unwrap();
            lvl1.get_mut(addr)
                .set_page(MaybeOwned::Owned(frame.map_alloc(GlobalPageAllocator)))
                .set_flags(flags);
        }

        let kstack_base_virt = kbase + KSTACK_SIZE - 0x1000;

        let interrupt_frame = InterruptStackFrameValue::new(
            VirtAddr::from_ptr(entry_point),
            process.privilege.get_code_segment(),
            RFlags::INTERRUPT_FLAG,
            VirtAddr::new(stack_base + STACK_SIZE),
            process.privilege.get_data_segment(),
        );

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
                    sp: kstack_base_virt as usize,
                    ip: start_new_task as *const () as usize,
                }),
                kstack_top: VirtAddr::from_ptr((kbase + KSTACK_SIZE) as *const ()),
                cr3_page: process.memory.lock().region.get_cr3() as u64,
                sse_state: None,
            }),
        });

        threads.threads.insert(tid, thread.clone());

        Some(thread)
    }

    pub fn thread(&self) -> Arc<Thread> {
        self.weak_self.upgrade().unwrap()
    }

    pub fn process(&self) -> &Arc<Process> {
        &self.process
    }

    pub fn tid(&self) -> Tid {
        self.tid
    }

    /// SAFETY: Must hold the global sched lock
    pub fn sched_global(&self) -> *mut ThreadSchedGlobalData {
        self.sched_global.0.get()
    }

    pub fn sched(&self) -> &Spinlock<ThreadSched> {
        &self.sched
    }

    pub fn wake(&self) {
        let mut s = self.sched.lock();
        match s.state {
            ThreadState::Zombie | ThreadState::Runnable | ThreadState::Killed => (),
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

impl Default for ThreadSchedGlobal {
    fn default() -> Self {
        Self::new()
    }
}

impl ThreadSchedGlobal {
    pub const fn new() -> Self {
        Self(UnsafeCell::new(ThreadSchedGlobalData::new()))
    }
}

pub struct ThreadSched {
    pub state: ThreadState,
    pub task_state: Option<SavedTaskState>,
    pub kstack_top: VirtAddr,
    pub cr3_page: u64,
    pub sse_state: Option<Box<[u8]>>,
}

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum ThreadState {
    Zombie,
    Runnable,
    Sleeping,
    Killed,
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
    VMO(Arc<Spinlock<VMO>>),
}

impl Debug for KernelValue {
    fn fmt(&self, f: &mut core::fmt::Formatter<'_>) -> core::fmt::Result {
        match self {
            Self::Message(_) => f.debug_tuple("KernelValue::Message").finish(),
            Self::Process(_) => f.debug_tuple("KernelValue::Process").finish(),
            Self::Channel(_) => f.debug_tuple("KernelValue::Channel").finish(),
            Self::Port(_) => f.debug_tuple("KernelValue::Port").finish(),
            Self::Interrupt(_) => f.debug_tuple("KernelValue::Interrupt").finish(),
            Self::VMO(_) => f.debug_tuple("KernelValue::VMO").finish(),
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
            KernelValue::VMO(_) => KernelObjectType::VMO,
        }
    }
}

impl From<Arc<KMessage>> for KernelValue {
    fn from(val: Arc<KMessage>) -> Self {
        KernelValue::Message(val)
    }
}

impl From<Arc<Process>> for KernelValue {
    fn from(val: Arc<Process>) -> Self {
        KernelValue::Process(val)
    }
}

impl From<Arc<KChannelHandle>> for KernelValue {
    fn from(val: Arc<KChannelHandle>) -> Self {
        KernelValue::Channel(val)
    }
}

impl From<Arc<KPort>> for KernelValue {
    fn from(val: Arc<KPort>) -> Self {
        KernelValue::Port(val)
    }
}

impl From<Arc<KInterruptHandle>> for KernelValue {
    fn from(val: Arc<KInterruptHandle>) -> Self {
        KernelValue::Interrupt(val)
    }
}

impl From<Arc<Spinlock<VMO>>> for KernelValue {
    fn from(val: Arc<Spinlock<VMO>>) -> Self {
        KernelValue::VMO(val)
    }
}

impl Drop for Thread {
    fn drop(&mut self) {
        let stack_base = STACK_ADDR + (STACK_SIZE * 2) * self.tid.into_raw();

        unsafe {
            self.process
                .memory
                .lock()
                .region
                .unmap(stack_base as usize, STACK_SIZE as usize)
                .unwrap();
            let ktop = self.sched.lock().kstack_top.as_u64();
            let mut lvl3 = KERNEL_STACKS_MAP.lock();
            let lvl3 = lvl3.as_mut();
            for page in (ktop - KSTACK_SIZE..ktop).step_by(0x1000) {
                let addr = page as usize;
                let EntryMut::Table(lvl2) = lvl3.get_mut(addr).entry_mut() else {
                    panic!();
                };
                let EntryMut::Table(lvl1) = lvl2.get_mut(addr).entry_mut() else {
                    panic!();
                };
                lvl1.get_mut(addr).take_page().unwrap();
                lvl2.get_mut(addr).gc_table();
                lvl3.get_mut(addr).gc_table();
            }
        }
    }
}
