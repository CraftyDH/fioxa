use core::ptr::slice_from_raw_parts;

use alloc::{boxed::Box, collections::BTreeMap, sync::Arc};

use kernel_userspace::{ids::ProcessID, process::ProcessExit, syscall::thread_bootstraper};
use spin::{Lazy, Mutex};
use x86_64::structures::idt::InterruptStackFrame;

use crate::{
    assembly::registers::{jump_to_userspace, Registers},
    cpu_localstorage::CPULocalStorageRW,
    kassert,
    syscall::SyscallError,
    time::pit::is_switching_tasks,
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
    // enable interrupts and wait for multitasking to start
    core::arch::asm!("sti");
    while !is_switching_tasks() {
        core::arch::asm!("hlt");
    }

    let task = CPULocalStorageRW::get_current_task();

    // jump into the nop task address space
    jump_to_userspace(task.state())
}

/// Used for sleeping each core after the task queue becomes empty
/// Aka the end of the round robin cycle
/// Or when an async task (like interrupts) need to be in an actual process to dispatch (to avoid deadlocks)
/// This reduces CPU load normally (doesn't thrash every core to 100%)
/// However is does reduce performance when there are actually tasks that could use the time
pub unsafe extern "C" fn nop_task() -> ! {
    // Init complete, start executing tasks
    CPULocalStorageRW::set_stay_scheduled(false);

    loop {
        // nothing to do so sleep
        unsafe { core::arch::asm!("hlt") };
    }
}

fn get_next_task() -> Box<Thread> {
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
            super::process::ThreadStatus::Blocked(_) => {
                panic!("a thread in the queue should not be blocked")
            }
        };
    }
}

pub unsafe fn save_current_task(stack_frame: &InterruptStackFrame, reg: &Registers) {
    let mut thread = CPULocalStorageRW::take_current_task();
    thread.save_state(stack_frame, reg);

    if CPULocalStorageRW::get_current_pid() == ProcessID(0) {
        CPULocalStorageRW::set_coremgmt_task(thread);
    } else {
        TASK_QUEUE.lock().push(thread);
    }
}

pub unsafe fn load_new_task(stack_frame: &mut InterruptStackFrame, reg: &mut Registers) {
    let thread = get_next_task();

    // If we are switching processes update page tables
    if CPULocalStorageRW::get_current_pid() != thread.process().pid {
        unsafe {
            thread
                .process()
                .memory
                .lock()
                .page_mapper
                .get_mapper_mut()
                .load_into_cr3()
        }
        CPULocalStorageRW::set_current_pid(thread.process().pid);
    }

    thread.restore_state(stack_frame, reg);
    CPULocalStorageRW::set_current_tid(thread.handle().tid());
    CPULocalStorageRW::set_current_task(thread);
    CPULocalStorageRW::set_ticks_left(5);
}

/// Kills the current task and jumps into a new one
/// DO NOT HOLD ANYTHING BEFORE CALLING THIS
pub fn kill_bad_task() -> ! {
    {
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

        let thread = get_next_task();

        if CPULocalStorageRW::get_current_pid() != thread.process().pid {
            unsafe {
                thread
                    .process()
                    .memory
                    .lock()
                    .page_mapper
                    .get_mapper_mut()
                    .load_into_cr3()
            }
            CPULocalStorageRW::set_current_pid(thread.process().pid);
        }

        CPULocalStorageRW::set_current_tid(thread.handle().tid());
        CPULocalStorageRW::set_current_task(thread);
        CPULocalStorageRW::set_ticks_left(5);
    };

    // jump into the nop task address space
    unsafe { jump_to_userspace(CPULocalStorageRW::get_current_task().state()) }
}

pub unsafe fn switch_task(stack_frame: &mut InterruptStackFrame, reg: &mut Registers) {
    save_current_task(stack_frame, reg);
    load_new_task(stack_frame, reg);
}

pub unsafe fn exit_thread(stack_frame: &mut InterruptStackFrame, reg: &mut Registers) {
    let thread = CPULocalStorageRW::take_current_task();
    exit_thread_inner(thread);
    load_new_task(stack_frame, reg);
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
    _stack_frame: &mut InterruptStackFrame,
    reg: &mut Registers,
) -> Result<(), SyscallError> {
    let curr = unsafe { CPULocalStorageRW::get_current_task() };

    kassert!(
        curr.process().privilege == super::process::ProcessPrivilige::KERNEL,
        "Only kernel may use spawn process"
    );

    let nbytes = unsafe { &*slice_from_raw_parts(reg.r9 as *const u8, reg.r10) };

    let privilege = if reg.r11 == 1 {
        super::process::ProcessPrivilige::KERNEL
    } else {
        super::process::ProcessPrivilige::USER
    };

    let process = Process::new(privilege, nbytes);
    let pid = process.pid;

    // TODO: Validate r8 is a valid entrypoint
    let thread = process.new_thread(thread_bootstraper as *const u64, reg.r8);
    PROCESSES.lock().insert(process.pid, process);
    push_task_queue(thread.expect("new process shouldn't have died"));

    // Return process id as successful result;
    reg.rax = pid.0 as usize;
    Ok(())
}

pub unsafe fn spawn_thread(stack_frame: &mut InterruptStackFrame, reg: &mut Registers) {
    let thread = CPULocalStorageRW::get_current_task();

    // TODO: Validate r8 is a valid entrypoint
    let thread = thread.process().new_thread(reg.r8 as *const u64, reg.r9);
    match thread {
        Some(thread) => {
            // Return process id as successful result;
            reg.rax = thread.handle().tid().0 as usize;
            push_task_queue(thread)
        }
        // process has been killed
        None => unsafe { exit_thread(stack_frame, reg) },
    }
}

pub unsafe fn yield_now(stack_frame: &mut InterruptStackFrame, reg: &mut Registers) {
    save_current_task(stack_frame, reg);
    load_new_task(stack_frame, reg);
}
