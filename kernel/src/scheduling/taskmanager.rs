use core::ptr::slice_from_raw_parts;

use alloc::{
    collections::BTreeMap,
    string::{String, ToString},
};

use crossbeam_queue::ArrayQueue;
use spin::Mutex;
use x86_64::structures::idt::InterruptStackFrame;

use crate::{
    assembly::registers::Registers,
    cpu_localstorage::{
        get_current_cpu_id, get_task_mgr_current_pid, get_task_mgr_current_tid,
        set_task_mgr_current_pid, set_task_mgr_current_ticks, set_task_mgr_current_tid,
    },
    paging::{
        page_table_manager::{PageLvl4, PageTable},
        virt_addr_for_phys,
    },
    stream::STREAMRef,
};

use super::process::{Process, Thread, PID, TID};

lazy_static::lazy_static! {
    pub static ref TASKMANAGER: Mutex<TaskManager> = Mutex::new(TaskManager::uninit());
}

pub struct TaskManager {
    core_cnt: u8,
    pub task_queue: ArrayQueue<(PID, TID)>,
    pub processes: BTreeMap<PID, Process>,
}

pub unsafe fn core_start_multitasking() -> ! {
    // Performs work to keep core working & is preemptible
    loop {
        core::arch::asm!("sti; hlt; pause");
    }
}

impl TaskManager {
    fn uninit() -> Self {
        Self {
            core_cnt: 0,
            task_queue: ArrayQueue::new(100),
            processes: BTreeMap::new(),
        }
    }

    // Can only be called once
    pub unsafe fn init(&mut self, mapper: PageTable<'static, PageLvl4>, core_cnt: u8) {
        self.core_cnt = core_cnt;
        let mut p = Process::new_with_page(
            crate::scheduling::process::ProcessPrivilige::KERNEL,
            mapper,
            "".into(),
        );
        assert!(p.pid == 0.into());

        for _ in 0..core_cnt {
            unsafe { p.new_overide_thread() };
        }
        self.processes.insert(p.pid, p);
    }

    fn get_next_task(&mut self, core_id: usize) -> (PID, TID) {
        // Get a new tasks if available
        if let Some(task) = self.task_queue.pop() {
            return task;
        }

        // If no tasks send into core mgmt
        (0.into(), (core_id as u64).into())
    }

    fn get_thread(&self, pid: PID, tid: TID) -> Option<(&Process, &Thread)> {
        let process = self.processes.get(&pid)?;
        let thread = process.threads.get(&tid)?;
        Some((process, thread))
    }

    fn get_thread_mut(&mut self, pid: PID, tid: TID) -> Option<&mut Thread> {
        let process = self.processes.get_mut(&pid)?;
        let thread = process.threads.get_mut(&tid)?;
        Some(thread)
    }

    fn save_current_task(
        &mut self,
        stack_frame: &mut InterruptStackFrame,
        reg: &mut Registers,
    ) -> Option<()> {
        let pid = get_task_mgr_current_pid();
        let tid = get_task_mgr_current_tid();
        let thread = self.get_thread_mut(pid, tid)?;
        thread.save(stack_frame, reg);
        // Don't save nop task
        if pid != 0.into() {
            self.task_queue.push((pid, tid)).unwrap()
        }
        Some(())
    }

    fn load_new_task(&mut self, stack_frame: &mut InterruptStackFrame, reg: &mut Registers) {
        let current_cpu = get_current_cpu_id() as usize;
        // Loop becuase we don't delete tasks from queue when they exit
        loop {
            let (pid, tid) = self.get_next_task(current_cpu);
            if let Some((p, thread)) = self.get_thread(pid, tid) {
                unsafe {
                    let cr3 = p.page_mapper.get_lvl4_addr() - virt_addr_for_phys(0);
                    core::arch::asm!(
                        "mov cr3, {}",
                        in(reg) cr3,
                    );
                }
                thread.restore(stack_frame, reg);
                set_task_mgr_current_pid(pid);
                set_task_mgr_current_tid(tid);
                set_task_mgr_current_ticks(5);
                return;
            }
        }
    }

    pub fn switch_task(&mut self, stack_frame: &mut InterruptStackFrame, reg: &mut Registers) {
        self.save_current_task(stack_frame, reg);
        self.load_new_task(stack_frame, reg);
    }

    pub fn exit_thread(&mut self, stack_frame: &mut InterruptStackFrame, reg: &mut Registers) {
        let pid = get_task_mgr_current_pid();
        let tid = get_task_mgr_current_tid();
        let process = self.processes.get_mut(&pid).unwrap();
        process.threads.remove(&tid).unwrap();
        if process.threads.is_empty() {
            self.processes.remove(&pid);
        }
        self.load_new_task(stack_frame, reg);
    }

    pub fn spawn_process(&mut self, _stack_frame: &mut InterruptStackFrame, reg: &mut Registers) {
        let nbytes = unsafe { &*slice_from_raw_parts(reg.r9 as *const u8, reg.r10) };
        let args = String::from_utf8_lossy(nbytes).to_string();

        let mut process = Process::new(super::process::ProcessPrivilige::KERNEL, args);
        let pid = process.pid;

        // TODO: Validate r8 is a valid entrypoint
        let thread = process.new_thread(reg.r8);
        self.processes.insert(pid, process);
        self.task_queue.push((pid, thread)).unwrap();
        // Return process id as successful result;
        reg.rax = pid.into();
    }

    pub fn spawn_thread(
        &mut self,
        _stack_frame: &mut InterruptStackFrame,
        reg: &mut Registers,
    ) -> Option<()> {
        let pid = get_task_mgr_current_pid();
        let process = self.processes.get_mut(&pid)?;

        // TODO: Validate r8 is a valid entrypoint
        let thread = process.new_thread(reg.r8);
        self.task_queue.push((pid, thread)).unwrap();
        // Return task id as successful result;
        reg.rax = thread.into();
        Some(())
    }

    pub fn yield_now(&mut self, stack_frame: &mut InterruptStackFrame, reg: &mut Registers) {
        self.save_current_task(stack_frame, reg);
        self.load_new_task(stack_frame, reg);
    }
}

extern "C" {
    pub fn nop_task();
}
