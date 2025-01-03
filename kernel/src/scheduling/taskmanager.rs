use core::fmt::Write;

use alloc::{boxed::Box, collections::BTreeMap, fmt, sync::Arc};

use conquer_once::spin::Lazy;
use kernel_userspace::{
    ids::ProcessID,
    object::{KernelReference, ObjectSignal},
    process::ProcessExit,
    syscall::thread_bootstraper,
};

use crate::{
    assembly::{registers::SavedTaskState, wrmsr},
    cpu_localstorage::CPULocalStorageRW,
    gdt::{KERNEL_CODE_SELECTOR, USER_CODE_SELECTOR},
    mutex::{Spinlock, SpinlockGuard},
    scheduling::{process::ThreadState, with_held_interrupts},
    syscall::{syscall_sysret_handler, SyscallError},
};

use super::process::{Process, Thread, ThreadSched};

pub type ProcessesListType = BTreeMap<ProcessID, Arc<Process>>;
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

impl ThreadSchedGlobalData {
    pub const fn new() -> Self {
        Self {
            queued: false,
            next: None,
        }
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
                head = &h.sched_global().next;
            }
            Ok(())
        }
    }

    fn pop_thread(&mut self) -> Option<Arc<Thread>> {
        unsafe {
            let head = self.queue_head.take()?;
            let sg = head.sched_global();
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
            let sg = thread.sched_global();
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
                    let tsg = tail.sched_global();
                    assert!(tsg.next.is_none());
                    tsg.next = Some(thread.clone());
                }
                self.queue_tail = Some(thread)
            }
        }
    }
}

pub unsafe fn enable_syscall() {
    // set up syscall/syscret
    // In Long Mode, userland CS will be loaded from STAR 63:48 + 16 and userland SS from STAR 63:48 + 8 on SYSRET.
    let star = ((USER_CODE_SELECTOR.0 - 16) as u64) << 48 | (KERNEL_CODE_SELECTOR.0 as u64) << 32;
    // set star
    wrmsr(0xC0000081, star);

    // set lstar (the rip that it'll go to)
    wrmsr(0xC0000082, syscall_sysret_handler as u64);

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

pub unsafe fn core_start_multitasking() {
    enable_syscall();

    // Init complete, start executing tasks
    CPULocalStorageRW::dec_hold_interrupts();

    core::arch::asm!(
        "sti",
        "mov rsp, gs:1",
        "jmp {}",
        sym scheduler,
    )
}

unsafe extern "C" fn scheduler() {
    let id = CPULocalStorageRW::get_core_id();
    info!("Starting scheduler on core: {}", id);

    loop {
        let task = SCHEDULER.lock().pop_thread();
        if let Some(task) = task {
            let mut sched = task.sched().lock();
            if !sched.in_syscall && sched.killed {
                exit_thread_inner(&task, &mut sched);
                continue;
            }
            assert_eq!(sched.state, ThreadState::Runnable);

            sched_run_tick(&task, &mut sched);

            if CPULocalStorageRW::hold_interrupts_depth() != 1 {
                error!("Thread shouldn't be holding interrupts when yielding");
                exit_thread_inner(&task, &mut sched);
                CPULocalStorageRW::set_hold_interrupts_depth(0);
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
                ThreadState::Sleeping => (),
            }
        } else {
            // nothing can run so sleep
            core::arch::asm!("hlt")
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
        }

        let mut sched = CPULocalStorageRW::get_current_task().sched().lock();
        sched.killed = true;
        if sched.in_syscall {
            error!("Killing bad task in syscall");
            sched.in_syscall = false;
        }
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
        *p.exit_status.lock() = ProcessExit::Exited;
        p.signals
            .lock()
            .set_signal(ObjectSignal::PROCESS_EXITED, true);
        PROCESSES.lock().remove(&p.pid);
    }
}

pub fn spawn_process<F>(
    func: F,
    args: &[u8],
    references: &[KernelReference],
    name: &'static str,
    kernel: bool,
) -> ProcessID
where
    F: Fn() + Send + Sync + 'static,
{
    let privilege = if kernel {
        super::process::ProcessPrivilige::KERNEL
    } else {
        super::process::ProcessPrivilige::USER
    };

    let process = Process::new(privilege, args, name);

    with_held_interrupts(|| unsafe {
        let mut refs = process.references.lock();
        let this = CPULocalStorageRW::get_current_task();
        let mut this_refs = this.process().references.lock();
        for r in references {
            refs.add_value(
                this_refs
                    .references()
                    .get(&r.id())
                    .expect("loader proc should have ref in its map")
                    .clone(),
            );
        }
    });

    let pid = process.pid;

    let boxed_func: Box<dyn Fn()> = Box::new(func);
    let raw = Box::into_raw(Box::new(boxed_func)) as usize;

    // TODO: Validate r8 is a valid entrypoint
    let thread = process.new_thread(thread_bootstraper as *const u64, raw);
    PROCESSES.lock().insert(process.pid, process);
    SCHEDULER.lock().queue_thread(thread.unwrap());

    // Return process id as successful result;
    pid
}

pub unsafe fn spawn_thread(arg1: usize, arg2: usize) -> Result<usize, SyscallError> {
    let thread = CPULocalStorageRW::get_current_task();

    // TODO: Validate r8 is a valid entrypoint
    let thread = thread.process().new_thread(arg1 as *const u64, arg2);
    match thread {
        Some(thread) => {
            // Return process id as successful result;
            let res = thread.tid().0 as usize;
            SCHEDULER.lock().queue_thread(thread);
            Ok(res)
        }
        // process has been killed
        None => todo!(),
    }
}

unsafe fn sched_run_tick(task: &Thread, sched: &mut ThreadSched) {
    let SavedTaskState { sp, ip } = sched.task_state.take().unwrap();

    let tss = &mut CPULocalStorageRW::get_gdt().tss;
    tss.privilege_stack_table[0] = sched.kstack_top;

    let cr3 = task.process().cr3_page;

    CPULocalStorageRW::set_current_task(task, &sched);

    let new_sp;
    let new_ip;

    core::arch::asm!(
        "push rbx",
        "push rbp",
        "pushfq",
        "mov r9, cr3", // save current cr3
        "push r9",
        "mov cr3, r8",
        "mov gs:0x9, cl",
        "lea r8, [rip+2f]",
        "mov gs:0x22, rsp",
        "mov gs:0x2A, r8",
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
    );
    CPULocalStorageRW::clear_current_task();

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
            "mov gs:0x9, cl",
            "lea rdi, [rip+2f]", // ret addr
            "mov rsi, rsp",      // save rsp
            "mov rsp, gs:0x22",  // load new stack
            "mov rax, gs:0x2A",
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
        );
    }
}
