use core::{mem::ManuallyDrop, ptr::slice_from_raw_parts};

use alloc::{boxed::Box, collections::BTreeMap, sync::Arc};

use kernel_userspace::{ids::ProcessID, process::ProcessExit, syscall::thread_bootstraper};
use spin::{mutex::SpinMutexGuard, Lazy, Mutex};
use x86_64::instructions::interrupts::without_interrupts;

use crate::{
    assembly::{registers::SavedTaskState, wrmsr},
    cpu_localstorage::CPULocalStorageRW,
    gdt::{KERNEL_CODE_SELECTOR, TASK_SWITCH_INDEX, USER_CODE_SELECTOR},
    kassert,
    syscall::{syscall_sysret_handler, SyscallError},
};

use super::{
    process::{LinkedThreadList, Process, Thread},
    without_context_switch,
};

pub type ProcessesListType = BTreeMap<ProcessID, Arc<Process>>;
pub static PROCESSES: Lazy<Mutex<ProcessesListType>> = Lazy::new(|| Mutex::new(BTreeMap::new()));
static TASK_QUEUE: Mutex<LinkedThreadList> = Mutex::new(LinkedThreadList::new());

pub fn push_task_queue(val: Box<Thread>) {
    without_context_switch(|| TASK_QUEUE.lock().push(val))
}

pub fn append_task_queue(list: &mut LinkedThreadList) {
    without_context_switch(|| TASK_QUEUE.lock().append(list))
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
    CPULocalStorageRW::set_stay_scheduled(false);

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
        let t = without_interrupts(|| TASK_QUEUE.lock().pop());
        match t {
            Some(task) => without_interrupts(|| {
                if !task.in_syscall
                    && task
                        .handle()
                        .kill_signal
                        .load(core::sync::atomic::Ordering::Relaxed)
                {
                    exit_thread_inner(task);
                    return;
                }
                let (task, res) = sched_run_tick(task);

                if res == ACTION_YIELD {
                    TASK_QUEUE.lock().push(task);
                } else if res == ACTION_EXIT_TASK {
                    exit_thread_inner(task);
                } else if res == ACTION_BLOCKING {
                    let handle = task.handle().clone();
                    *handle.thread.as_mut_ptr() = Some(task);
                    handle.thread.force_unlock();
                } else {
                    panic!("should be a valid action")
                }
            }),
            None => {
                info!("putting core {id} to sleep");
                // nothing can run so sleep
                core::arch::asm!("hlt")
            }
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
            thread.handle().tid(),
            thread.process().privilege
        );

        if CPULocalStorageRW::get_context() == 0 {
            panic!("Cannot kill in context 0");
        }
        exit_task();
    }
}

pub fn exit_task() -> ! {
    unsafe {
        enter_sched(ACTION_EXIT_TASK);
        unreachable!("exit thread shouldn't return")
    }
}

pub fn exit_thread_inner(thread: Box<Thread>) {
    let p = thread.process();
    let mut t = p.threads.lock();
    t.threads
        .remove(&thread.handle().tid())
        .expect("thread should be in thread list");
    if t.threads.is_empty() {
        drop(t);
        *p.exit_status.lock() = ProcessExit::Exited;
        p.exit_signal.lock().set_level(true);
        PROCESSES.lock().remove(&p.pid);
    }
}

pub fn spawn_process(
    arg1: usize,
    arg2: usize,
    arg3: usize,
    arg4: usize,
) -> Result<usize, SyscallError> {
    let curr = unsafe { CPULocalStorageRW::get_current_task() };

    kassert!(
        curr.process().privilege != super::process::ProcessPrivilige::USER,
        "Only kernel may use spawn process"
    );

    let nbytes = unsafe { &*slice_from_raw_parts(arg2 as *const u8, arg3) };

    let privilege = if arg4 == 1 {
        super::process::ProcessPrivilige::KERNEL
    } else {
        super::process::ProcessPrivilige::USER
    };

    let process = Process::new(privilege, nbytes);
    let pid = process.pid;

    // TODO: Validate r8 is a valid entrypoint
    let thread = process.new_thread(thread_bootstraper as *const u64, arg1);
    PROCESSES.lock().insert(process.pid, process);
    push_task_queue(thread.expect("new process shouldn't have died"));

    // Return process id as successful result;
    Ok(pid.0 as usize)
}

pub unsafe fn spawn_thread(arg1: usize, arg2: usize) -> Result<usize, SyscallError> {
    let thread = CPULocalStorageRW::get_current_task();

    // TODO: Validate r8 is a valid entrypoint
    let thread = thread.process().new_thread(arg1 as *const u64, arg2);
    match thread {
        Some(thread) => {
            // Return process id as successful result;
            let res = thread.handle().tid().0 as usize;
            push_task_queue(thread);
            Ok(res)
        }
        // process has been killed
        None => todo!(),
    }
}

pub const ACTION_YIELD: usize = 0;
pub const ACTION_EXIT_TASK: usize = 1;
pub const ACTION_BLOCKING: usize = 2;

unsafe fn sched_run_tick(mut task: Box<Thread>) -> (Box<Thread>, usize) {
    let SavedTaskState { sp, ip, saved_arg } = task.state.take().unwrap();

    let tss = &mut CPULocalStorageRW::get_gdt().tss;
    let saved_switch = tss.interrupt_stack_table[TASK_SWITCH_INDEX as usize];
    tss.interrupt_stack_table[TASK_SWITCH_INDEX as usize] = task.kstack_top;

    let cr3 = task
        .process()
        .memory
        .lock()
        .page_mapper
        .get_mapper_mut()
        .into_page()
        .get_address();

    CPULocalStorageRW::set_current_task(task);

    let new_sp;
    let new_ip;
    let action;

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
        in("rax") saved_arg,
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
        lateout("rdx") action,
        lateout("rcx") _,
    );
    let mut task = CPULocalStorageRW::take_current_task();

    tss.interrupt_stack_table[TASK_SWITCH_INDEX as usize] = saved_switch;

    task.state = Some(SavedTaskState {
        sp: new_sp,
        ip: new_ip,
        saved_arg: 0,
    });

    (task, action)
}

pub unsafe fn enter_sched(action: usize) -> usize {
    assert!(
        !CPULocalStorageRW::get_stay_scheduled(),
        "Thread should not be asking to stay scheduled and also enter the scheduler."
    );

    let res;
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
        in("rdx") action,
        lateout("rax") res,
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
    res
}

pub fn yield_task() {
    unsafe { enter_sched(ACTION_YIELD) };
}

pub fn block_task(handle: SpinMutexGuard<Option<Box<Thread>>>) -> usize {
    let _ = ManuallyDrop::new(handle);
    unsafe { enter_sched(ACTION_BLOCKING) }
}
