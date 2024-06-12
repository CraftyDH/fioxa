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
    let task = CPULocalStorageRW::get_current_task();

    task.state.take().unwrap().jump();
}

/// Used for sleeping each core after the task queue becomes empty
/// Aka the end of the round robin cycle
/// Or when an async task (like interrupts) need to be in an actual process to dispatch (to avoid deadlocks)
/// This reduces CPU load normally (doesn't thrash every core to 100%)
/// However is does reduce performance when there are actually tasks that could use the time
pub unsafe extern "C" fn nop_task() -> ! {
    // enable interrupts and wait for multitasking to start
    core::arch::asm!("sti");
    while !is_switching_tasks() {
        core::arch::asm!("hlt");
    }

    // Init complete, start executing tasks
    CPULocalStorageRW::set_stay_scheduled(false);

    loop {
        // nothing to do so sleep
        unsafe { core::arch::asm!("hlt") };
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
                return task;
            }
            super::process::ThreadStatus::PleaseKill => {
                exit_thread_inner(task);
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
pub fn kill_bad_task() -> ! {
    // switch stack
    unsafe { core::arch::asm!("mov rsp, gs:1") }
    let thread = {
        let thread = CPULocalStorageRW::take_current_task();

        warn!(
            "KILLING BAD TASK: PID: {:?}, TID: {:?}",
            thread.process().pid,
            thread.handle().tid()
        );

        match thread.process().privilege {
            crate::scheduling::process::ProcessPrivilige::KERNEL => {
                panic!("STOPPING CORE AS KERNEL CANNOT DO BAD")
            }
            crate::scheduling::process::ProcessPrivilige::USER => (),
        }

        exit_thread_inner(thread);
        get_next_task()
    };

    unsafe { thread.switch_to() }
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
        curr.process().privilege == super::process::ProcessPrivilige::KERNEL,
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
    // save rbx, rbp and make llvm save anything it cares about
    core::arch::asm!(
        "push rbx",
        "push rbp",
        "lea rdi, [rip+2f]", // ret addr
        "mov rsi, rsp",      // save rsp
        "mov rsp, gs:1",     // load new stack
        "jmp rax",
        "2:",
        "pop rbp",
        "pop rbx",
        in("rax") callback,
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

pub unsafe extern "C" fn yield_task(saved_rip: usize, saved_rsp: usize) -> ! {
    let mut task = CPULocalStorageRW::take_current_task();
    task.state = Some(SavedTaskState {
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
        // if kill return because we can't leave kernel threads without cleanup
        super::process::ThreadStatus::PleaseKill => {
            drop(status);
            state.jump();
        }
        // was blocking so start blocking
        super::process::ThreadStatus::Blocking => {
            let mut task = CPULocalStorageRW::take_current_task();
            task.state = Some(state);
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
