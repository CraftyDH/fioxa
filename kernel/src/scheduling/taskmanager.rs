use core::{ptr::slice_from_raw_parts, sync::atomic::AtomicBool};

use alloc::{
    collections::BTreeMap,
    sync::{Arc, Weak},
    vec::Vec,
};

use crossbeam_queue::ArrayQueue;
use kernel_userspace::ids::ProcessID;
use spin::{Lazy, Mutex};
use x86_64::structures::idt::InterruptStackFrame;

use crate::{
    assembly::registers::Registers, cpu_localstorage::CPULocalStorageRW,
    interrupts::check_interrupts, time::pit::is_switching_tasks,
};

use super::{
    process::{Process, Thread},
    without_context_switch,
};

pub type ProcessesListType = BTreeMap<ProcessID, Arc<Process>>;
pub static PROCESSES: Lazy<Mutex<ProcessesListType>> = Lazy::new(|| Mutex::new(BTreeMap::new()));
static TASK_QUEUE: Lazy<ArrayQueue<Weak<Thread>>> = Lazy::new(|| ArrayQueue::new(1000));

static GO_INTO_CORE_MGMT: AtomicBool = AtomicBool::new(false);

pub fn push_task_queue(val: Weak<Thread>) -> Result<(), Weak<Thread>> {
    without_context_switch(|| TASK_QUEUE.push(val))
}

#[inline(always)]
pub fn enter_core_mgmt() {
    GO_INTO_CORE_MGMT.store(true, core::sync::atomic::Ordering::Relaxed);
}

/// Used for sleeping each core after the task queue becomes empty
/// Aka the end of the round robin cycle
/// Or when an async task (like interrupts) need to be in an actual process to dispatch (to avoid deadlocks)
/// This reduces CPU load normally (doesn't thrash every core to 100%)
/// However is does reduce performance when there are actually tasks that could use the time
pub unsafe fn core_start_multitasking() -> ! {
    // enable interrupts and wait for multitasking to start
    core::arch::asm!("sti");
    while !is_switching_tasks() {
        core::arch::asm!("hlt");
    }

    // Init complete, start executing tasks
    CPULocalStorageRW::set_stay_scheduled(false);

    let mut buf = Vec::new();
    loop {
        // Check interrupts
        if check_interrupts(&mut buf) {
            // try scheduling another task
            kernel_userspace::syscall::yield_now();
        } else {
            // no interrupts to handle so sleep
            unsafe { core::arch::asm!("hlt") };
        }
    }
}

fn get_next_task() -> Arc<Thread> {
    // if there is a task that needs core mgmt
    if GO_INTO_CORE_MGMT.swap(false, core::sync::atomic::Ordering::Relaxed) {
        return CPULocalStorageRW::get_core_mgmt_task();
    }

    // get the next available task or run core mgmt
    loop {
        if let Some(task) = TASK_QUEUE.pop() {
            if let Some(t) = task.upgrade() {
                return t;
            }
            // if the task died, try and find a new task
        } else {
            // If no tasks send into core mgmt
            return CPULocalStorageRW::get_core_mgmt_task();
        }
    }
}

fn save_current_task(stack_frame: &InterruptStackFrame, reg: &Registers) {
    let thread = CPULocalStorageRW::get_current_task();
    thread.context.lock().save(stack_frame, reg);

    // Don't save nop task
    if CPULocalStorageRW::get_current_pid() != ProcessID(0) {
        TASK_QUEUE.push(Arc::downgrade(&thread)).unwrap();
    }
}

pub fn load_new_task(stack_frame: &mut InterruptStackFrame, reg: &mut Registers) {
    let thread = get_next_task();

    // If we are switching processes update page tables
    if CPULocalStorageRW::get_current_pid() != thread.process.pid {
        unsafe {
            thread
                .process
                .memory
                .lock()
                .page_mapper
                .get_mapper_mut()
                .load_into_cr3()
        }
        CPULocalStorageRW::set_current_pid(thread.process.pid);
    }

    thread.context.lock().restore(stack_frame, reg);
    CPULocalStorageRW::set_current_tid(thread.tid);
    CPULocalStorageRW::set_current_task(thread);
    CPULocalStorageRW::set_ticks_left(5);
}

pub fn switch_task(stack_frame: &mut InterruptStackFrame, reg: &mut Registers) {
    save_current_task(stack_frame, reg);
    load_new_task(stack_frame, reg);
}

pub fn exit_thread(stack_frame: &mut InterruptStackFrame, reg: &mut Registers) {
    {
        let thread = CPULocalStorageRW::get_current_task();
        let p = &thread.process;
        let mut t = p.threads.lock();
        t.threads
            .remove(&thread.tid)
            .expect("thread should be in thread list");
        if t.threads.is_empty() {
            PROCESSES.lock().remove(&p.pid);
        }
    }

    load_new_task(stack_frame, reg);
}

pub fn spawn_process(_stack_frame: &mut InterruptStackFrame, reg: &mut Registers) {
    let nbytes = unsafe { &*slice_from_raw_parts(reg.r9 as *const u8, reg.r10) };

    let privilege = if reg.r11 == 1 {
        super::process::ProcessPrivilige::KERNEL
    } else {
        super::process::ProcessPrivilige::USER
    };

    let process = Process::new(privilege, nbytes);
    let pid = process.pid;

    // TODO: Validate r8 is a valid entrypoint
    let thread = process.new_thread(reg.r8);
    PROCESSES.lock().insert(process.pid, process);
    TASK_QUEUE.push(Arc::downgrade(&thread)).unwrap();
    // Return process id as successful result;
    reg.rax = pid.0 as usize;
}

pub fn spawn_thread(_stack_frame: &mut InterruptStackFrame, reg: &mut Registers) {
    let thread = CPULocalStorageRW::get_current_task();

    // TODO: Validate r8 is a valid entrypoint
    let thread = thread.process.new_thread(reg.r8);
    TASK_QUEUE.push(Arc::downgrade(&thread)).unwrap();
    // Return task id as successful result;
    reg.rax = thread.tid.0 as usize;
}

pub fn yield_now(stack_frame: &mut InterruptStackFrame, reg: &mut Registers) {
    save_current_task(stack_frame, reg);
    load_new_task(stack_frame, reg);
}
