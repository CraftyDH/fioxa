use core::{
    any::type_name,
    arch::x86_64::{
        __cpuid, __cpuid_count, _fxrstor64, _fxsave64, _xrstor64, _xrstors64, _xsave64,
        _xsaveopt64, _xsaves64,
    },
    fmt::Write,
    panic,
};

use alloc::{boxed::Box, collections::BTreeMap, fmt, sync::Arc};

use kernel_sys::{
    syscall::sys_thread_bootstrapper,
    types::{ObjectSignal, Pid, Tid},
};
use spin::{Lazy, Once};
use x86_64::{
    instructions::interrupts,
    registers::{
        control::{Cr0, Cr0Flags, Cr4, Cr4Flags},
        xcontrol::{XCr0, XCr0Flags},
    },
};

use crate::{
    assembly::{registers::SavedTaskState, wrmsr},
    cpu_localstorage::{CPULocalStorage, CPULocalStorageRW},
    gdt::{KERNEL_CODE_SELECTOR, USER_CODE_SELECTOR},
    mutex::{Spinlock, SpinlockGuard},
    scheduling::process::ThreadState,
    syscall::syscall_sysret_handler,
};

use super::process::{Process, ProcessBuilder, ProcessMemory, Thread, ThreadSched};

pub type ProcessesListType = BTreeMap<Pid, Arc<Process>>;
pub static PROCESSES: Lazy<Spinlock<ProcessesListType>> =
    Lazy::new(|| Spinlock::new(BTreeMap::new()));

pub static SCHEDULER: Spinlock<GlobalSchedData> = Spinlock::new(GlobalSchedData::new());

pub struct GlobalSchedData {
    queue_head: Option<Arc<Thread>>,
    queue_tail: Option<Arc<Thread>>,
}

pub struct ThreadSchedGlobalData {
    queued: bool,
    next: Option<Arc<Thread>>,
}

impl Default for ThreadSchedGlobalData {
    fn default() -> Self {
        Self::new()
    }
}

impl ThreadSchedGlobalData {
    pub const fn new() -> Self {
        Self {
            queued: false,
            next: None,
        }
    }
}

impl Default for GlobalSchedData {
    fn default() -> Self {
        Self::new()
    }
}

impl GlobalSchedData {
    pub const fn new() -> Self {
        Self {
            queue_head: None,
            queue_tail: None,
        }
    }

    pub fn dump_runnable(&self, writer: &mut impl Write) -> fmt::Result {
        unsafe {
            writer.write_str("Runnable tasks\n")?;
            let mut head = &self.queue_head;
            while let Some(h) = head {
                writer.write_fmt(format_args!("{h:?}\n"))?;
                head = &(*h.sched_global()).next;
            }
            Ok(())
        }
    }

    fn pop_thread(&mut self) -> Option<Arc<Thread>> {
        unsafe {
            let head = self.queue_head.take()?;
            let sg = &mut *head.sched_global();
            sg.queued = false;
            match sg.next.take() {
                nxt @ Some(_) => self.queue_head = nxt,
                None => {
                    // We were head and tail
                    self.queue_tail = None;
                }
            }
            Some(head)
        }
    }

    pub fn queue_thread(&mut self, thread: Arc<Thread>) {
        unsafe {
            let sg = &mut *thread.sched_global();
            if sg.queued {
                return;
            }
            sg.queued = true;

            if self.queue_head.is_none() {
                // Case 1: nothing else is in the queue, we become head and tail
                assert!(self.queue_tail.is_none());
                self.queue_head = Some(thread.clone());
                self.queue_tail = Some(thread)
            } else {
                // Case 2: insert ourself as the new tail
                if let Some(tail) = self.queue_tail.take() {
                    let tsg = &mut *tail.sched_global();
                    assert!(tsg.next.is_none());
                    tsg.next = Some(thread.clone());
                }
                self.queue_tail = Some(thread)
            }
        }
    }
}

pub unsafe fn enable_syscall() {
    unsafe {
        // set up syscall/syscret
        // In Long Mode, userland CS will be loaded from STAR 63:48 + 16 and userland SS from STAR 63:48 + 8 on SYSRET.
        let star =
            ((USER_CODE_SELECTOR.0 - 16) as u64) << 48 | (KERNEL_CODE_SELECTOR.0 as u64) << 32;
        // set star
        wrmsr(0xC0000081, star);

        // set lstar (the rip that it'll go to)
        wrmsr(0xC0000082, syscall_sysret_handler as *const () as u64);

        // set flag mask (mask everything)
        wrmsr(0xC0000084, 0x200);

        // enable syscall
        core::arch::asm!(
            "rdmsr",
            "or eax, 1",
            "wrmsr",
            in("ecx") 0xC0000080u32,
            lateout("ecx") _,
            lateout("eax") _,
            options(preserves_flags, nostack)
        );
    }
}

#[derive(Debug, Clone)]
#[repr(C)]
enum SSESaveMethod {
    FxSave,
    XSave,
    XSaveOpt,
    XSaves,
}

#[derive(Debug, Clone)]
struct SSESave {
    default: Box<[u8]>,
    method: SSESaveMethod,
}

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
#[repr(C)]
#[allow(clippy::upper_case_acronyms)]
enum SSEType {
    SSE,
    AVX,
    AVX512,
}

unsafe fn enable_sse_inner() -> SSEType {
    unsafe {
        let cpu = __cpuid(0x1);
        assert!(cpu.edx & (1 << 25) > 0, "SSE baseline support is required");
        let mut cr0 = Cr0::read();
        cr0.remove(Cr0Flags::EMULATE_COPROCESSOR);
        cr0.insert(Cr0Flags::MONITOR_COPROCESSOR);
        Cr0::write(cr0);
        let mut cr4 = Cr4::read();
        cr4.insert(Cr4Flags::OSFXSR | Cr4Flags::OSXMMEXCPT_ENABLE);
        Cr4::write(cr4);

        let cpu = __cpuid(0x1);
        // AVX | XSAVE
        let vals = (1 << 28) | (1 << 26);
        if (cpu.ecx & vals) != vals {
            return SSEType::SSE;
        }

        // OSXSAVE requires XSAVE to be supported
        let mut cr4 = Cr4::read();
        cr4.insert(Cr4Flags::OSXSAVE);
        Cr4::write(cr4);
        // Check that OSXSAVE is supported
        // This doesn't seem to be set until cr4 is set.
        let cpu = __cpuid(0x1);
        if (cpu.ecx & (1 << 27)) == 0 {
            return SSEType::SSE;
        }

        // enable AVX
        let mut xcr0 = XCr0::read();
        xcr0.insert(XCr0Flags::AVX | XCr0Flags::SSE | XCr0Flags::X87);
        XCr0::write(xcr0);

        // Enable AVX512 if possible
        let cpu = __cpuid(0xD);
        let vals = (1 << 5) | (1 << 6) | (1 << 7);
        if cpu.eax & vals != vals {
            return SSEType::AVX;
        }
        let mut xcr0 = XCr0::read();
        xcr0.insert(XCr0Flags::OPMASK | XCr0Flags::ZMM_HI256 | XCr0Flags::HI16_ZMM);
        XCr0::write(xcr0);

        SSEType::AVX512
    }
}

unsafe fn sse_avx_get_save() -> SSESave {
    unsafe {
        let extended_state_main = __cpuid_count(0xD, 0);
        let extended_state_leaf1 = __cpuid_count(0xD, 1);

        // Prefer xsaves > xsaveopt > xsave
        // TODO: Do we want to use xsavec?
        let (default, method) = if extended_state_leaf1.eax & 0b1000 > 0 {
            let mut default =
                Box::new_zeroed_slice(extended_state_leaf1.ebx as usize).assume_init();
            _xsaves64(default.as_mut_ptr(), u64::MAX);
            (default, SSESaveMethod::XSaves)
        } else if extended_state_leaf1.eax & 0b1 > 0 {
            let mut default = Box::new_zeroed_slice(extended_state_main.ebx as usize).assume_init();
            _xsaveopt64(default.as_mut_ptr(), u64::MAX);
            (default, SSESaveMethod::XSaveOpt)
        } else {
            let mut default = Box::new_zeroed_slice(extended_state_main.ebx as usize).assume_init();
            _xsave64(default.as_mut_ptr(), u64::MAX);
            (default, SSESaveMethod::XSave)
        };
        info!("AVX Save type {method:?}");
        SSESave { default, method }
    }
}

unsafe fn sse_sse_get_save() -> SSESave {
    unsafe {
        let mut default = Box::new([0; 512]);
        _fxsave64(default.as_mut_ptr());
        SSESave {
            default,
            method: SSESaveMethod::FxSave,
        }
    }
}

unsafe fn sse_get_method(ty: SSEType) -> SSESave {
    unsafe {
        match ty {
            SSEType::SSE => sse_sse_get_save(),
            SSEType::AVX | SSEType::AVX512 => sse_avx_get_save(),
        }
    }
}

static SSE_TYPE: Once<(SSEType, SSESave)> = Once::new();

unsafe fn enable_sse() -> &'static SSESave {
    unsafe {
        let ty = enable_sse_inner();
        let (global_ty, method) = SSE_TYPE.call_once(|| {
            info!("SSE Type: {ty:?}");
            (ty, sse_get_method(ty))
        });
        assert_eq!(ty, *global_ty, "Each core should have the same SSE type");
        method
    }
}

pub unsafe fn reset_sse() {
    let sse = &SSE_TYPE.get().unwrap().1;
    unsafe {
        match sse.method {
            SSESaveMethod::FxSave => _fxrstor64(sse.default.as_ptr()),
            SSESaveMethod::XSave | SSESaveMethod::XSaveOpt => {
                _xrstor64(sse.default.as_ptr(), u64::MAX)
            }
            SSESaveMethod::XSaves => _xrstors64(sse.default.as_ptr(), u64::MAX),
        }
    }
}

pub unsafe fn core_start_multitasking() {
    unsafe {
        enable_syscall();

        // Init complete, start executing tasks
        CPULocalStorageRW::dec_hold_interrupts();

        core::arch::asm!(
            "sti",
            "mov rsp, gs:{stack}",
            "nop",
            "jmp {}",
            sym scheduler,
            stack = const core::mem::offset_of!(CPULocalStorage, stack_top),
        )
    }
}

unsafe extern "C" fn scheduler() {
    let id = CPULocalStorageRW::get_core_id();
    info!("Starting scheduler on core: {id}");

    let sse = unsafe { enable_sse() };

    loop {
        let task = SCHEDULER.lock().pop_thread();
        if let Some(task) = task {
            let mut sched = task.sched().lock();

            assert_eq!(sched.state, ThreadState::Runnable);

            unsafe { sched_run_tick(&task, &mut sched, sse) };

            if CPULocalStorageRW::hold_interrupts_depth() != 1 {
                error!("Thread shouldn't be holding interrupts when yielding");
                exit_thread_inner(&task, &mut sched);
                CPULocalStorageRW::set_hold_interrupts_depth(1);
                continue;
            }

            match sched.state {
                ThreadState::Zombie => {
                    panic!("bad state")
                }
                ThreadState::Runnable => {
                    sched.state = ThreadState::Runnable;
                    drop(sched);
                    SCHEDULER.lock().queue_thread(task);
                }
                ThreadState::Killed => {
                    exit_thread_inner(&task, &mut sched);
                }
                ThreadState::Sleeping => (),
            }
        } else {
            // nothing can run so sleep
            unsafe { core::arch::asm!("hlt") };
        }

        if CPULocalStorageRW::hold_interrupts_depth() != 0 {
            warn!("interrupts?");
        }

        if !interrupts::are_enabled() {
            warn!("interrupts??");
        }
    }
}

/// Kills the current task and jumps into a new one
/// DO NOT HOLD ANYTHING BEFORE CALLING THIS
pub fn kill_bad_task() -> ! {
    unsafe {
        let thread = CPULocalStorageRW::get_current_task();

        warn!(
            "KILLING BAD TASK: PID: {:?}, TID: {:?}, PRIV: {:?}",
            thread.process().pid,
            thread.tid(),
            thread.process().privilege
        );

        if CPULocalStorageRW::get_context() == 0 {
            panic!("Cannot kill in context 0");
        } else if CPULocalStorageRW::get_context() == 1 {
            error!("Killing bad task in syscall");
        }

        let mut sched = CPULocalStorageRW::get_current_task().sched().lock();
        sched.state = ThreadState::Killed;

        enter_sched(&mut sched);
        unreachable!("exit thread shouldn't return");
    }
}

pub fn exit_thread_inner(thread: &Thread, sched: &mut ThreadSched) {
    sched.state = ThreadState::Zombie;
    let p = thread.process();
    let mut t = p.threads.lock();
    if t.threads.remove(&thread.tid()).is_none() {
        error!("thread should be in thread list {thread:?}")
    }

    if t.threads.is_empty() {
        drop(t);
        *p.exit_status.lock() = Some(1);
        p.signals
            .lock()
            .set_signal(ObjectSignal::PROCESS_EXITED, true);
        PROCESSES.lock().remove(&p.pid);
    }
}

pub fn spawn_process<F>(func: F) -> ProcessBuilder
where
    F: FnOnce() + Send + Sync + 'static,
{
    let boxed_func: Box<dyn FnOnce()> = Box::new(func);
    let raw = Box::into_raw(Box::new(boxed_func)) as usize;

    ProcessBuilder::new(
        ProcessMemory::new().unwrap(),
        sys_thread_bootstrapper as *const u64,
        raw,
    )
    .privilege(super::process::ProcessPrivilege::KERNEL)
    .name(type_name::<F>())
}

pub unsafe fn spawn_thread(arg1: usize, arg2: usize) -> Tid {
    let thread = unsafe { CPULocalStorageRW::get_current_task() };

    // TODO: Validate r8 is a valid entrypoint
    let thread = Thread::new(thread.process().clone(), arg1 as *const u64, arg2);
    match thread {
        Some(thread) => {
            // Return process id as successful result;
            let tid = thread.tid();
            SCHEDULER.lock().queue_thread(thread);
            tid
        }
        // process has been killed
        None => todo!(),
    }
}

unsafe fn sched_run_tick(task: &Thread, sched: &mut ThreadSched, sse: &SSESave) {
    let SavedTaskState { sp, ip } = sched.task_state.take().unwrap();

    let tss = &mut CPULocalStorageRW::get_gdt().tss;
    tss.privilege_stack_table[0] = sched.kstack_top;

    let cr3 = sched.cr3_page;

    CPULocalStorageRW::set_current_task(task, sched);

    let new_sp;
    let new_ip;

    let sse_region = sched.sse_state.get_or_insert_with(|| sse.default.clone());

    unsafe {
        match sse.method {
            SSESaveMethod::FxSave => _fxrstor64(sse_region.as_ptr()),
            SSESaveMethod::XSave | SSESaveMethod::XSaveOpt => {
                _xrstor64(sse_region.as_ptr(), u64::MAX)
            }
            SSESaveMethod::XSaves => _xrstors64(sse_region.as_ptr(), u64::MAX),
        }

        core::arch::asm!(
            "push rbx",
            "push rbp",
            "pushfq",
            "mov r9, cr3", // save current cr3
            "push r9",
            "mov cr3, r8",
            "mov gs:{ctx}, cl",
            "lea r8, [rip+2f]",
            "mov gs:{sp}, rsp",
            "mov gs:{ip}, r8",
            "mov rsp, rsi",
            "jmp rdi",
            "2:",
            "pop rax",
            "mov cr3, rax",
            "popfq",
            "pop rbp",
            "pop rbx",
            in("cl") 1u8,
            in("rdi") ip,
            in("rsi") sp,
            in("r8") cr3,
            lateout("rax") _,
            lateout("r15") _,
            lateout("r14") _,
            lateout("r13") _,
            lateout("r12") _,
            lateout("r11") _,
            lateout("r10") _,
            lateout("r9") _,
            lateout("r8") _,
            lateout("rdi") new_ip,
            lateout("rsi") new_sp,
            lateout("rdx") _,
            lateout("rcx") _,
            ctx = const core::mem::offset_of!(CPULocalStorage, current_context),
            sp = const core::mem::offset_of!(CPULocalStorage, sched_task_sp),
            ip = const core::mem::offset_of!(CPULocalStorage, sched_task_ip),
        );
        match sse.method {
            SSESaveMethod::FxSave => _fxsave64(sse_region.as_mut_ptr()),
            SSESaveMethod::XSave => _xsave64(sse_region.as_mut_ptr(), u64::MAX),
            SSESaveMethod::XSaveOpt => _xsaveopt64(sse_region.as_mut_ptr(), u64::MAX),
            SSESaveMethod::XSaves => _xsaves64(sse_region.as_mut_ptr(), u64::MAX),
        }
    };
    CPULocalStorageRW::clear_current_task();

    // we want interrupts to be enabled in the scheduler once the counter drops to zero,
    // and by running tick a interrupt handler could have set this to false
    CPULocalStorageRW::set_hold_interrupts_initial(true);

    sched.task_state = Some(SavedTaskState {
        sp: new_sp,
        ip: new_ip,
    });
}

/// We need to hold the threads spinlock before enter, and it will be held after return
pub fn enter_sched(_: &mut SpinlockGuard<ThreadSched>) {
    unsafe {
        core::arch::asm!(
            "push rbx",
            "push rbp",
            "pushfq",
            "cli",
            "mov gs:{ctx}, cl",
            "lea rdi, [rip+2f]", // ret addr
            "mov rsi, rsp",      // save rsp
            "mov rsp, gs:{sp}",  // load new stack
            "mov rax, gs:{ip}",
            "jmp rax",
            "2:",
            "popfq",
            "pop rbp",
            "pop rbx",
            in("cl") 0u8,
            lateout("rax") _,
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
            lateout("rdx") _,
            lateout("rcx") _,
            ctx = const core::mem::offset_of!(CPULocalStorage, current_context),
            sp = const core::mem::offset_of!(CPULocalStorage, sched_task_sp),
            ip = const core::mem::offset_of!(CPULocalStorage, sched_task_ip),
        );
    }
}
