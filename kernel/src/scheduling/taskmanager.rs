use core::{ptr::slice_from_raw_parts, sync::atomic::AtomicBool};

use alloc::{collections::BTreeMap, vec::Vec};

use conquer_once::noblock::OnceCell;
use crossbeam_queue::ArrayQueue;
use kernel_userspace::ids::{ProcessID, ThreadID};
use spin::{Lazy, Mutex};
use x86_64::structures::idt::InterruptStackFrame;

use crate::{
    assembly::registers::Registers,
    cpu_localstorage::CPULocalStorageRW,
    interrupts::check_interrupts,
    paging::{
        page_table_manager::{PageLvl4, PageTable},
        virt_addr_for_phys,
    },
};

use super::{
    process::{Process, Thread},
    without_context_switch,
};

pub static PROCESSES: Lazy<Mutex<BTreeMap<ProcessID, Process>>> =
    Lazy::new(|| Mutex::new(BTreeMap::new()));
static TASK_QUEUE: Lazy<ArrayQueue<(ProcessID, ThreadID)>> = Lazy::new(|| ArrayQueue::new(1000));

pub static CORE_COUNT: OnceCell<u8> = OnceCell::uninit();

static GO_INTO_CORE_MGMT: AtomicBool = AtomicBool::new(false);

pub fn push_task_queue(val: (ProcessID, ThreadID)) -> Result<(), (ProcessID, ThreadID)> {
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
    // Performs work to keep core working & is preemptible
    CPULocalStorageRW::set_stay_scheduled(false);
    core::arch::asm!("sti");

    let mut buf = Vec::new();
    loop {
        // Check interrupts
        if check_interrupts(&mut buf) {
            kernel_userspace::syscall::yield_now();
        } else {
            // no interrupts to handle so sleep
            core::arch::asm!("hlt");
        }
    }
}

fn get_next_task(core_id: usize) -> (ProcessID, ThreadID) {
    // if there is a task that needs core mgmt
    if GO_INTO_CORE_MGMT.swap(false, core::sync::atomic::Ordering::Relaxed) {
        return (ProcessID(0), ThreadID(core_id as u64));
    }

    // Get a new tasks if available
    if let Some(task) = TASK_QUEUE.pop() {
        return task;
    }

    // If no tasks send into core mgmt
    (ProcessID(0), ThreadID(core_id as u64))
}

fn get_thread(
    pid: ProcessID,
    tid: ThreadID,
    processes: &BTreeMap<ProcessID, Process>,
) -> Option<(&Process, &Thread)> {
    let process = processes.get(&pid)?;
    let thread = process.threads.get(&tid)?;
    Some((process, thread))
}

fn get_thread_mut<'a>(
    pid: ProcessID,
    tid: ThreadID,
    processes: &'a mut BTreeMap<ProcessID, Process>,
) -> Option<&'a mut Thread> {
    let process = processes.get_mut(&pid)?;
    let thread = process.threads.get_mut(&tid)?;
    Some(thread)
}

pub unsafe fn init(mapper: PageTable<'static, PageLvl4>, core_cnt: u8) {
    CORE_COUNT.try_init_once(|| core_cnt).unwrap();
    let mut p = Process::new_with_page(
        crate::scheduling::process::ProcessPrivilige::KERNEL,
        mapper,
        &[],
    );
    assert!(p.pid == ProcessID(0));

    for _ in 0..core_cnt {
        unsafe { p.new_overide_thread() };
    }
    PROCESSES.lock().insert(p.pid, p);
}

fn save_current_task(stack_frame: &mut InterruptStackFrame, reg: &mut Registers) -> Option<()> {
    let pid = CPULocalStorageRW::get_current_pid();
    let tid = CPULocalStorageRW::get_current_tid();
    {
        let mut processes = PROCESSES.lock();
        let thread = get_thread_mut(pid, tid, &mut processes)?;
        thread.save(stack_frame, reg);
    }
    // Don't save nop task
    if pid != ProcessID(0) {
        TASK_QUEUE.push((pid, tid)).unwrap();
    }
    Some(())
}

pub fn load_new_task(stack_frame: &mut InterruptStackFrame, reg: &mut Registers) {
    let current_cpu = CPULocalStorageRW::get_core_id() as usize;
    // Loop becuase we don't delete tasks from queue when they exit
    loop {
        let (pid, tid) = get_next_task(current_cpu);
        let processes = PROCESSES.lock();
        if let Some((p, thread)) = get_thread(pid, tid, &processes) {
            unsafe {
                let cr3 = p.page_mapper.get_lvl4_addr() - virt_addr_for_phys(0);
                core::arch::asm!(
                    "mov cr3, {}",
                    in(reg) cr3,
                );
            }
            thread.restore(stack_frame, reg);
            CPULocalStorageRW::set_current_pid(pid);
            CPULocalStorageRW::set_current_tid(tid);
            CPULocalStorageRW::set_ticks_left(5);
            return;
        }
    }
}

pub fn switch_task(stack_frame: &mut InterruptStackFrame, reg: &mut Registers) {
    save_current_task(stack_frame, reg).unwrap();
    load_new_task(stack_frame, reg);
}

pub fn exit_thread(stack_frame: &mut InterruptStackFrame, reg: &mut Registers) {
    let pid = CPULocalStorageRW::get_current_pid();
    let tid = CPULocalStorageRW::get_current_tid();

    {
        let mut processes = PROCESSES.lock();

        let process = processes.get_mut(&pid).unwrap();
        process.threads.remove(&tid).unwrap();
        if process.threads.is_empty() {
            processes.remove(&pid);
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

    let mut process = Process::new(privilege, nbytes);
    let pid = process.pid;

    // TODO: Validate r8 is a valid entrypoint
    let thread = process.new_thread(reg.r8);
    PROCESSES.lock().insert(pid, process);
    TASK_QUEUE.push((pid, thread)).unwrap();
    // Return process id as successful result;
    reg.rax = pid.0 as usize;
}

pub fn spawn_thread(_stack_frame: &mut InterruptStackFrame, reg: &mut Registers) {
    let pid = CPULocalStorageRW::get_current_pid();
    let mut p = PROCESSES.lock();
    let process = p.get_mut(&pid).unwrap();

    // TODO: Validate r8 is a valid entrypoint
    let thread = process.new_thread(reg.r8);
    TASK_QUEUE.push((pid, thread)).unwrap();
    // Return task id as successful result;
    reg.rax = thread.0 as usize;
}

pub fn yield_now(stack_frame: &mut InterruptStackFrame, reg: &mut Registers) {
    save_current_task(stack_frame, reg).unwrap();
    load_new_task(stack_frame, reg);
}
