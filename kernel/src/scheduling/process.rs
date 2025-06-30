use core::{
    cell::UnsafeCell,
    fmt::Debug,
    num::NonZeroUsize,
    ops::ControlFlow,
    sync::atomic::{AtomicU64, Ordering},
};

use alloc::{
    boxed::Box,
    collections::BTreeMap,
    sync::{Arc, Weak},
    vec::Vec,
};
use kernel_sys::{
    raw::syscall::{KernelSyscallHandlerBreak, SyscallHandler},
    types::{Hid, KernelObjectType, Pid, RawValue, Tid, VMMapFlags, VMOAnonymousFlags},
};
use num_derive::FromPrimitive;
use num_traits::FromPrimitive;
use slab::Slab;
use spin::Lazy;
use x86_64::{
    VirtAddr,
    instructions::interrupts::without_interrupts,
    registers::rflags::RFlags,
    structures::{gdt::SegmentSelector, idt::InterruptStackFrameValue},
};

use crate::{
    assembly::registers::Registers,
    channel::KChannelHandle,
    cpu_localstorage::{CPULocalStorage, CPULocalStorageRW},
    gdt,
    interrupts::KInterruptHandle,
    message::KMessage,
    mutex::Spinlock,
    object::{KObject, KObjectSignal},
    paging::{
        KERNEL_STACKS_MAP, MemoryLoc, PageAllocator,
        page::{Page, Size4KB},
        page_allocator::global_allocator,
        page_table::Mapper,
    },
    port::KPort,
    scheduling::taskmanager::enter_sched,
    syscall::KernelSyscallHandler,
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
    pub fn new() -> Self {
        let mut region = VirtualMemoryRegion::new(global_allocator());

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

        Self { region }
    }
}

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
            args: args,
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

pub enum VMEnterState {
    Full(VMCompleteState),
    IntSyscall(VMEnterIntSyscall),
    Syscall(VMEnterStateSyscall),
    Kernel(VMEnterKernelSyscall),
}

pub enum VMExitState {
    Complete(VMCompleteState),
    IntSyscall(VMExitStateIntSyscall),
    Syscall(VMExitStateSyscall),
    Kernel(VMExitKernelSyscall),
}

#[derive(Debug, FromPrimitive)]
pub enum VMExitStateID {
    Complete,
    IntSyscall,
    Syscall,
    Kernel,
}

#[derive(Clone)]
#[repr(C)]
pub struct VMCompleteState {
    pub regs: Registers,
    pub ret: InterruptStackFrameValue,
}

#[derive(Clone)]
#[repr(C)]
pub struct PreservedSyscallRegisters {
    pub rbx: usize,
    pub rbp: usize,
    pub r12: usize,
    pub r13: usize,
    pub r14: usize,
    pub r15: usize,
}

#[derive(Clone)]
#[repr(C)]
pub struct VMExitStateIntSyscall {
    pub args: [usize; 7],
    pub preserved: PreservedSyscallRegisters,
    pub ret: InterruptStackFrameValue,
}

#[derive(Clone)]
#[repr(C)]
pub struct VMEnterIntSyscall {
    pub result: usize,
    pub preserved: PreservedSyscallRegisters,
    pub ret: InterruptStackFrameValue,
}

#[derive(Clone)]
#[repr(C)]
pub struct VMExitStateSyscall {
    pub regs: [usize; 7],
    pub preserved: PreservedSyscallRegisters,
    pub rcx: usize, // RIP
    pub r11: usize, // RFLAGS
    pub rsp: usize,
}

#[derive(Clone)]
#[repr(C)]
pub struct VMEnterStateSyscall {
    pub result: usize,
    pub preserved: PreservedSyscallRegisters,
    pub rcx: usize, // RIP
    pub r11: usize, // RFLAGS
    pub rsp: usize,
}

#[derive(Clone)]
#[repr(C)]
pub struct VMEnterKernelSyscall {
    pub result: usize,
    pub ret_stack: usize,
}

#[derive(Clone)]
#[repr(C)]
pub struct VMExitKernelSyscall {
    pub regs: [usize; 7],
    pub ret_stack: usize,
}

pub unsafe fn run_vm_tick<'a>(state: &VMEnterState) -> VMExitState {
    without_interrupts(|| unsafe {
        let depth = CPULocalStorageRW::hold_interrupts_depth();
        let initial = CPULocalStorageRW::hold_interrupts_initial();
        CPULocalStorageRW::set_hold_interrupts_depth(0);

        CPULocalStorageRW::set_context(2);

        let mut res_type: usize;
        let mut res_ptr: usize;
        match state {
            VMEnterState::Full(state) => {
                core::arch::asm!(
                    "lea rax, [rip+2f]",
                    "push rbx",
                    "push rbp",
                    "pushfq",
                    "push rax",
                    "mov gs:{vm_sp}, rsp",

                    "mov rsp, rdi",
                    "pop r15",
                    "pop r14",
                    "pop r13",
                    "pop r12",
                    "pop r11",
                    "pop r10",
                    "pop r9",
                    "pop r8",
                    "pop rdi",
                    "pop rsi",
                    "pop rdx",
                    "pop rcx",
                    "pop rbx",
                    "pop rax",
                    "pop rbp",
                    "iretq",

                    "2:",
                    "popfq",
                    "pop rbp",
                    "pop rbx",
                    in("rdi") state,
                    lateout("rax") res_type,
                    lateout("rdx") res_ptr,
                    lateout("r15") _,
                    lateout("r14") _,
                    lateout("r13") _,
                    lateout("r12") _,
                    lateout("r11") _,
                    lateout("r10") _,
                    lateout("r9") _,
                    lateout("r8") _,
                    lateout("rdi") _,
                    lateout("rsi") _,
                    lateout("rcx") _,
                    vm_sp = const core::mem::offset_of!(CPULocalStorage, vm_exit_sp),
                );
            }
            VMEnterState::IntSyscall(state) => {
                core::arch::asm!(
                    "lea rax, [rip+2f]",
                    "push rbx",
                    "push rbp",
                    "pushfq",
                    "push rax",
                    "mov gs:{vm_sp}, rsp",

                    "mov rsp, rdi",
                    "pop rax", // result
                    "pop r15",
                    "pop r14",
                    "pop r13",
                    "pop r12",
                    "pop rbp",
                    "pop rbx",

                    // clear scratch registers
                    "xor r11d, r11d",
                    "xor r10d, r10d",
                    "xor r9d,  r9d",
                    "xor r8d,  r8d",
                    "xor edi,  edi",
                    "xor esi,  esi",
                    "xor edx,  edx",
                    "xor ecx,  ecx",
                    "iretq",

                    "2:",
                    "popfq",
                    "pop rbp",
                    "pop rbx",
                    in("rdi") state,
                    lateout("rax") res_type,
                    lateout("rdx") res_ptr,
                    lateout("r15") _,
                    lateout("r14") _,
                    lateout("r13") _,
                    lateout("r12") _,
                    lateout("r11") _,
                    lateout("r10") _,
                    lateout("r9") _,
                    lateout("r8") _,
                    lateout("rdi") _,
                    lateout("rsi") _,
                    lateout("rcx") _,
                    vm_sp = const core::mem::offset_of!(CPULocalStorage, vm_exit_sp),
                );
            }
            VMEnterState::Syscall(state) => {
                core::arch::asm!(
                    "lea rax, [rip+2f]",
                    "push rbx",
                    "push rbp",
                    "pushfq",
                    "push rax",
                    "mov gs:{vm_sp}, rsp",

                    "mov rsp, rdi",

                    // result
                    "pop rax",
                    // saved registers
                    "pop r15",
                    "pop r14",
                    "pop r13",
                    "pop r12",
                    "pop rbp",
                    "pop rbx",

                    // syscall registers
                    "pop rcx",
                    "pop r11",
                    "pop rsp",

                    // clear scratch registers
                    "xor r10d, r10d",
                    "xor r9d,  r9d",
                    "xor r8d,  r8d",
                    "xor edi,  edi",
                    "xor esi,  esi",
                    "xor edx,  edx",

                    "sysretq",

                    "2:",
                    "popfq",
                    "pop rbp",
                    "pop rbx",
                    in("rdi") state,
                    lateout("rax") res_type,
                    lateout("rdx") res_ptr,
                    lateout("r15") _,
                    lateout("r14") _,
                    lateout("r13") _,
                    lateout("r12") _,
                    lateout("r11") _,
                    lateout("r10") _,
                    lateout("r9") _,
                    lateout("r8") _,
                    lateout("rdi") _,
                    lateout("rsi") _,
                    lateout("rcx") _,
                    vm_sp = const core::mem::offset_of!(CPULocalStorage, vm_exit_sp),
                );
            }
            VMEnterState::Kernel(state) => {
                core::arch::asm!(
                    "lea rsi, [rip+2f]",
                    "push rbx",
                    "push rbp",
                    "pushfq",
                    "push rsi",
                    "mov gs:{vm_sp}, rsp",

                    "mov rsp, rdi",
                    "pop r15",
                    "pop r14",
                    "pop r13",
                    "pop r12",
                    "pop rbp",
                    "pop rbx",
                    "popfq",
                    "ret",

                    "2:",
                    "popfq",
                    "pop rbp",
                    "pop rbx",
                    in("rax") state.result,
                    in("rdi") state.ret_stack,
                    lateout("rax") res_type,
                    lateout("rdx") res_ptr,
                    lateout("r15") _,
                    lateout("r14") _,
                    lateout("r13") _,
                    lateout("r12") _,
                    lateout("r11") _,
                    lateout("r10") _,
                    lateout("r9") _,
                    lateout("r8") _,
                    lateout("rdi") _,
                    lateout("rsi") _,
                    lateout("rcx") _,
                    vm_sp = const core::mem::offset_of!(CPULocalStorage, vm_exit_sp),
                )
            }
        };
        CPULocalStorageRW::set_hold_interrupts_initial(initial);
        CPULocalStorageRW::set_hold_interrupts_depth(depth);
        CPULocalStorageRW::set_context(1);

        // We must read the states before we exit held interrupts or an interrupt could override them
        use VMExitStateID as ID;
        match ID::from_usize(res_type).expect("The vm exit type should be valid.") {
            ID::Complete => VMExitState::Complete(core::ptr::read_volatile(res_ptr as _)),
            ID::IntSyscall => VMExitState::IntSyscall(core::ptr::read_volatile(res_ptr as _)),
            ID::Syscall => VMExitState::Syscall(core::ptr::read_volatile(res_ptr as _)),
            ID::Kernel => VMExitState::Kernel(core::ptr::read_volatile(res_ptr as _)),
        }
    })
}

#[unsafe(naked)]
unsafe extern "C" fn thread_run_bootstrap() {
    /// We need extern "C" to pass the pointer in arg 1, but it was set by rust code so probably fine?
    #[allow(improper_ctypes_definitions)]
    extern "C" fn thread_runner(runner: &RawRunner) -> ! {
        unsafe { CPULocalStorageRW::set_context(1) };
        let thread = unsafe { CPULocalStorageRW::get_current_task() };

        // We must unlock the mutex after return from scheduler
        unsafe { thread.sched().force_unlock() };

        // get the stored runner
        let runner = unsafe { Box::from_raw(*runner) };
        (runner)(thread);

        let mut sched = thread.sched().lock();
        sched.state = ThreadState::Killed;
        enter_sched(&mut sched);
        unreachable!("exit thread shouldn't return")
    }

    core::arch::naked_asm!(
        "mov rdi, rsp",
        "jmp {runner}",
        runner = sym thread_runner
    )
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

type RawRunner = *mut (dyn FnOnce(&Thread) + 'static);

fn thread_runner(entry_point: *const u64, stack_base: u64, arg: usize) -> impl FnOnce(&Thread) {
    move |thread| {
        let mut state = VMEnterState::Full(VMCompleteState {
            regs: Registers {
                rdi: arg,
                ..Default::default()
            },
            ret: InterruptStackFrameValue::new(
                VirtAddr::from_ptr(entry_point),
                thread.process.privilege.get_code_segment(),
                RFlags::INTERRUPT_FLAG,
                VirtAddr::new(stack_base + STACK_SIZE),
                thread.process.privilege.get_data_segment(),
            ),
        });

        let mut handler = KernelSyscallHandler { thread };

        let mut run = || -> ControlFlow<KernelSyscallHandlerBreak, !> {
            loop {
                let res = unsafe { run_vm_tick(&state) };
                match res {
                    VMExitState::Complete(vmstate_yield) => {
                        state = VMEnterState::Full(vmstate_yield);

                        let mut sched = thread.sched().lock();
                        enter_sched(&mut sched);
                    }
                    VMExitState::IntSyscall(syscall) => {
                        state = VMEnterState::IntSyscall(VMEnterIntSyscall {
                            result: handler.handle(&syscall.args)?,
                            preserved: syscall.preserved,
                            ret: syscall.ret,
                        });
                    }
                    VMExitState::Syscall(syscall) => {
                        state = VMEnterState::Syscall(VMEnterStateSyscall {
                            result: handler.handle(&syscall.regs)?,
                            preserved: syscall.preserved,
                            rcx: syscall.rcx,
                            r11: syscall.r11,
                            rsp: syscall.rsp,
                        })
                    }
                    VMExitState::Kernel(syscall) => {
                        state = VMEnterState::Kernel(VMEnterKernelSyscall {
                            result: handler.handle(&syscall.regs)?,
                            ret_stack: syscall.ret_stack,
                        })
                    }
                }
            }
        };

        let ControlFlow::Break(exit) = run();

        match exit {
            KernelSyscallHandlerBreak::AssertFailed => (),
            KernelSyscallHandlerBreak::UnknownSyscall => warn!("unknown syscall"),
            KernelSyscallHandlerBreak::Exit => return,
        }

        warn!(
            "TASK EXITING FROM ERROR: PID: {:?}, TID: {:?}, PRIV: {:?}",
            thread.process().pid,
            thread.tid(),
            thread.process().privilege
        );
    }
}

impl Thread {
    pub fn new(process: Arc<Process>, entry_point: *const u64, arg: usize) -> Option<Arc<Thread>> {
        let mut threads = process.threads.lock();
        let tid = threads.get_next_id();

        // let stack_base = STACK_ADDR.fetch_add(0x1000_000, Ordering::Relaxed);
        let stack_base = STACK_ADDR + (STACK_SIZE + 0x1000) * tid.into_raw();

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
        for page in (kbase..kbase + KSTACK_SIZE).step_by(0x1000) {
            let frame = alloc.allocate_page().unwrap();
            KERNEL_STACKS_MAP
                .lock()
                .map(alloc, Page::new(page), frame, VMMapFlags::WRITEABLE)
                .unwrap()
                .ignore();
        }

        let mut kstack_top = (kbase + KSTACK_SIZE) as usize;

        let runner = Box::new(thread_runner(entry_point, stack_base, arg));

        unsafe {
            kstack_top -= size_of::<RawRunner>();
            *(kstack_top as *mut RawRunner) = Box::into_raw(runner);
            kstack_top -= size_of::<usize>();
            *(kstack_top as *mut usize) = thread_run_bootstrap as usize;
        };
        let thread = Arc::new_cyclic(|this| Thread {
            weak_self: this.clone(),
            process: process.clone(),
            tid,
            sched_global: ThreadSchedGlobal::new(),
            sched: Spinlock::new(ThreadSched {
                state: ThreadState::Runnable,
                saved_sp: Some(kstack_top),
                kstack_top: VirtAddr::from_ptr((kbase + KSTACK_SIZE) as *const ()),
                cr3_page: process.memory.lock().region.get_cr3() as u64,
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

    pub fn tid(&self) -> Tid {
        self.tid
    }

    /// SAFTEY: Must hold the global sched lock
    pub unsafe fn sched_global(&self) -> &mut ThreadSchedGlobalData {
        unsafe { &mut *self.sched_global.0.get() }
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

impl ThreadSchedGlobal {
    pub const fn new() -> Self {
        Self(UnsafeCell::new(ThreadSchedGlobalData::new()))
    }
}

pub struct ThreadSched {
    pub state: ThreadState,
    pub saved_sp: Option<usize>,
    pub kstack_top: VirtAddr,
    pub cr3_page: u64,
}

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum ThreadState {
    Zombie,
    Runnable,
    Sleeping,
    Killed,
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

impl Into<KernelValue> for Arc<Spinlock<VMO>> {
    fn into(self) -> KernelValue {
        KernelValue::VMO(self)
    }
}

impl Drop for Thread {
    fn drop(&mut self) {
        let stack_base = STACK_ADDR + (STACK_SIZE + 0x1000) * self.tid.into_raw();

        unsafe {
            self.process
                .memory
                .lock()
                .region
                .unmap(stack_base as usize, STACK_SIZE as usize)
                .unwrap();
            let alloc = global_allocator();
            let ktop = self.sched.lock().kstack_top.as_u64();
            for page in (ktop - KSTACK_SIZE..ktop).step_by(0x1000) {
                let page = Page::<Size4KB>::new(page);
                let mut m = KERNEL_STACKS_MAP.lock();
                let phys = m.address_of(page).unwrap();
                m.unmap(alloc, page).unwrap().flush();
                alloc.free_page(phys);
            }
        }
    }
}
