use core::ptr::slice_from_raw_parts;

use alloc::{boxed::Box, collections::BTreeMap, sync::Arc};

use kernel_userspace::{ids::ProcessID, process::ProcessExit, syscall::thread_bootstraper};
use spin::{Lazy, Mutex};

use crate::{
    assembly::registers::SavedTaskState, cpu_localstorage::CPULocalStorageRW, kassert,
    syscall::SyscallError, time::pit::is_switching_tasks,
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

pub unsafe fn core_start_multitasking() -> ! {
    let task = CPULocalStorageRW::take_coremgmt_task();
    task.switch_to();
}

/// Used for sleeping each core after the task queue becomes empty
/// Aka the end of the round robin cycle
/// Or when an async task (like interrupts) need to be in an actual process to dispatch (to avoid deadlocks)
/// This reduces CPU load normally (doesn't thrash every core to 100%)
/// However is does reduce performance when there are actually tasks that could use the time
pub extern "C" fn nop_task() {
    info!("Starting nop task");
    unsafe {
        // enable interrupts and wait for multitasking to start
        core::arch::asm!("sti");
        while !is_switching_tasks() {
            core::arch::asm!("hlt");
        }

        // Init complete, start executing tasks
        CPULocalStorageRW::set_stay_scheduled(false);

        loop {
            // nothing to do so sleep
            core::arch::asm!("hlt");
        }
    }
}

pub fn get_next_task() -> Box<Thread> {
    // get the next available task or run core mgmt
    loop {
        // we cannot hold task queue for long,
        // because exit_thread might trigger a notification that places a task into the queue
        let Some(task) = TASK_QUEUE.lock().pop() else {
            return CPULocalStorageRW::take_coremgmt_task();
        };
        let status = core::mem::take(&mut *task.handle().status.lock());
        match status {
            super::process::ThreadStatus::Ok => {
                if task
                    .handle()
                    .kill_signal
                    .load(core::sync::atomic::Ordering::Relaxed)
                {
                    exit_thread_inner(task);
                } else {
                    return task;
                }
            }
            super::process::ThreadStatus::Blocked(_)
            | super::process::ThreadStatus::Blocking
            | super::process::ThreadStatus::BlockingRet(_) => {
                panic!("a thread in the queue should not be blocked")
            }
        };
    }
}

pub fn queue_thread(thread: Box<Thread>) {
    if CPULocalStorageRW::get_current_pid() == ProcessID(0) {
        CPULocalStorageRW::set_coremgmt_task(thread);
    } else {
        TASK_QUEUE.lock().push(thread);
    }
}

/// Kills the current task and jumps into a new one
/// DO NOT HOLD ANYTHING BEFORE CALLING THIS
#[naked]
pub extern "C" fn kill_bad_task() -> ! {
    extern "C" fn inner() -> ! {
        let thread = {
            let thread = CPULocalStorageRW::take_current_task();

            warn!(
                "KILLING BAD TASK: PID: {:?}, TID: {:?}, PRIV: {:?}",
                thread.process().pid,
                thread.handle().tid(),
                thread.process().privilege
            );

            if thread.process().pid.0 == 0 {
                panic!("Cannot kill process 0");
            }

            exit_thread_inner(thread);
            get_next_task()
        };

        unsafe { thread.switch_to() }
    }
    // switch stack
    unsafe {
        core::arch::asm!(
            "cli",
            "mov rsp, gs:0xA",
            "xor eax, eax",
            "mov gs:0x9, al",
            "jmp {}",
            sym inner,
            options(noreturn)
        )
    };
}

pub unsafe extern "C" fn exit_thread(_: usize, _: usize) -> ! {
    let thread = CPULocalStorageRW::take_current_task();
    exit_thread_inner(thread);
    get_next_task().switch_to();
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

pub unsafe fn block_task(callback: unsafe extern "C" fn(usize, usize) -> !) -> usize {
    let res;

    assert!(
        !CPULocalStorageRW::get_stay_scheduled(),
        "Thread should not be asking to stay scheduled and block."
    );

    // save rbx, rbp, flags and make llvm save anything it cares about
    core::arch::asm!(
        "push rbx",
        "push rbp",
        "pushfq",
        "mov gs:0x9, dil",
        "lea rdi, [rip+2f]", // ret addr
        "mov rsi, rsp",      // save rsp
        "mov rsp, gs:0xA",     // load new stack
        "call rax",
        "2:",
        "popfq",
        "pop rbp",
        "pop rbx",
        in("rax") callback,
        in("dil") 0u8,
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

pub unsafe extern "C" fn yield_task(saved_rip: usize, saved_rsp: usize) -> ! {
    let mut task = CPULocalStorageRW::take_current_task();
    task.save(SavedTaskState {
        sp: saved_rsp,
        ip: saved_rip,
        saved_arg: 0,
    });

    queue_thread(task);
    get_next_task().switch_to();
}

pub unsafe extern "C" fn switch_task(saved_rip: usize, saved_rsp: usize) -> ! {
    let task = CPULocalStorageRW::get_current_task();
    let mut state = SavedTaskState {
        sp: saved_rsp,
        ip: saved_rip,
        saved_arg: 0,
    };

    let mut status = task.handle().status.lock();

    match &mut *status {
        // was blocking so start blocking
        super::process::ThreadStatus::Blocking => {
            let mut task = CPULocalStorageRW::take_current_task();
            task.save(state);
            *status = super::process::ThreadStatus::Blocked(task);
            drop(status);
            get_next_task().switch_to();
        }
        // something triggered wake up so return
        super::process::ThreadStatus::BlockingRet(val) => {
            state.saved_arg = *val;
            *status = super::process::ThreadStatus::Ok;
            drop(status);
            state.jump();
        }
        _ => panic!("bad state"),
    }
}
