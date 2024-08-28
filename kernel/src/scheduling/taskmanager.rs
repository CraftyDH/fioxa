use core::{mem::ManuallyDrop, ptr::slice_from_raw_parts};

use alloc::{boxed::Box, collections::BTreeMap, sync::Arc};

use kernel_userspace::{ids::ProcessID, process::ProcessExit, syscall::thread_bootstraper};
use spin::{mutex::SpinMutexGuard, Lazy, Mutex};

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

pub unsafe fn core_start_multitasking() -> ! {
    enable_syscall();

    let mut target = CPULocalStorageRW::take_coremgmt_task();

    let state = target.state.take().unwrap();

    // switch contexts
    unsafe {
        target
            .process()
            .memory
            .lock()
            .page_mapper
            .get_mapper_mut()
            .load_into_cr3_lazy()
    }
    CPULocalStorageRW::get_gdt().tss.interrupt_stack_table[TASK_SWITCH_INDEX as usize] =
        target.kstack_top;
    CPULocalStorageRW::set_current_task(target);

    state.jump();
}

/// Used for sleeping each core after the task queue becomes empty
/// Aka the end of the round robin cycle
/// Or when an async task (like interrupts) need to be in an actual process to dispatch (to avoid deadlocks)
/// This reduces CPU load normally (doesn't thrash every core to 100%)
/// However is does reduce performance when there are actually tasks that could use the time
pub extern "C" fn nop_task() {
    info!("Starting nop task: {}", CPULocalStorageRW::get_core_id());
    unsafe {
        // enable interrupts and wait for multitasking to start
        core::arch::asm!("sti");

        // Init complete, start executing tasks
        CPULocalStorageRW::set_stay_scheduled(false);

        loop {
            // nothing to do so sleep
            core::arch::asm!("hlt");
        }
    }
}

pub fn get_next_task() -> Option<Box<Thread>> {
    // get the next available task or run core mgmt
    loop {
        // we cannot hold task queue for long,
        // because exit_thread might trigger a notification that places a task into the queue
        let task = TASK_QUEUE.lock().pop()?;
        if !task.in_syscall
            && task
                .handle()
                .kill_signal
                .load(core::sync::atomic::Ordering::Relaxed)
        {
            exit_thread_inner(task);
        } else {
            return Some(task);
        }
    }
}

pub fn get_next_task_always() -> Box<Thread> {
    get_next_task().unwrap_or_else(|| {
        let thread = CPULocalStorageRW::take_coremgmt_task();
        thread
    })
}

pub fn queue_thread(thread: Box<Thread>) {
    if thread.process().pid == ProcessID(0) {
        CPULocalStorageRW::set_coremgmt_task(thread);
    } else {
        TASK_QUEUE.lock().push(thread);
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

        if thread.process().pid.0 == 0 {
            panic!("Cannot kill process 0");
        }
        exit_task();
    }
}

pub fn exit_task() -> ! {
    unsafe {
        context_switch(get_next_task_always(), exit_thread_callback);
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

pub unsafe extern "C" fn context_switch_helper(
    target: *mut Thread,
    save_callback: extern "C" fn(Box<Thread>),
    return_rsp: usize,
    return_rip: usize,
) -> ! {
    let mut target = Box::from_raw(target);

    let mut current_task = CPULocalStorageRW::take_current_task();
    current_task.save(SavedTaskState {
        sp: return_rsp,
        ip: return_rip,
        saved_arg: 0,
    });

    let state = target.state.take().unwrap();

    // switch contexts
    unsafe {
        target
            .process()
            .memory
            .lock()
            .page_mapper
            .get_mapper_mut()
            .load_into_cr3_lazy()
    }
    CPULocalStorageRW::get_gdt().tss.interrupt_stack_table[TASK_SWITCH_INDEX as usize] =
        target.kstack_top;
    CPULocalStorageRW::set_current_task(target);

    // now that everythings been switched save current task
    save_callback(current_task);

    state.jump();
}

pub extern "C" fn queue_task_callback(task: Box<Thread>) {
    queue_thread(task)
}

pub unsafe fn context_switch(
    target: Box<Thread>,
    save_callback: unsafe extern "C" fn(Box<Thread>),
) -> usize {
    assert!(
        !CPULocalStorageRW::get_stay_scheduled(),
        "Thread should not be asking to stay scheduled and block."
    );

    let target = Box::into_raw(target);

    let res;

    // save rbx, rbp, flags and make llvm save anything it cares about
    core::arch::asm!(
        "push rbx",
        "push rbp",
        "pushfq",
        "mov gs:0x9, cl",
        "lea rcx, [rip+2f]", // ret addr
        "mov rdx, rsp",      // save rsp
        "mov rsp, gs:0xA",   // load new stack
        "call rax",
        "2:",
        "popfq",
        "pop rbp",
        "pop rbx",
        in("rdi") target,
        in("rsi") save_callback,
        in("rax") context_switch_helper,
        in("cl") 0u8,
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
        options(preserves_flags),
    );
    res
}

pub fn yield_task() {
    if let Some(t) = get_next_task() {
        unsafe {
            context_switch(t, queue_task_callback);
        }
    }
}

pub unsafe extern "C" fn exit_thread_callback(thread: Box<Thread>) {
    exit_thread_inner(thread);
}

pub fn block_task(handle: SpinMutexGuard<Option<Box<Thread>>>) -> usize {
    let _ = ManuallyDrop::new(handle);
    unsafe { context_switch(get_next_task_always(), block_task_callback) }
}

pub unsafe extern "C" fn block_task_callback(task: Box<Thread>) {
    let handle = task.handle().clone();
    *handle.thread.as_mut_ptr() = Some(task);
    handle.thread.force_unlock();
}
