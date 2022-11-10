use alloc::{collections::BTreeMap, vec::Vec};

use crossbeam_queue::SegQueue;
use spin::Mutex;
use x86_64::structures::idt::InterruptStackFrame;

use crate::{
    assembly::registers::Registers, cpu_localstorage::get_current_cpu_id,
    paging::page_table_manager::PageTableManager,
};

use super::process::{Process, Thread, PID, TID};

lazy_static::lazy_static! {
    static ref PROCESSOR_QUEUE: [SegQueue<(PID, TID)>; 8] = [0; 8].map(|_| SegQueue::new());
}

pub static TASKMANAGER: Mutex<TaskManager> = Mutex::new(TaskManager::uninit());

pub struct TaskManager {
    core_cnt: u8,
    task_queue: SegQueue<(PID, TID)>,
    core_current_task: Vec<(PID, TID)>,
    processes: BTreeMap<PID, Process>,
}

pub unsafe fn core_start_multitasking() -> ! {
    // Performs work to keep core working & is preemptible
    loop {
        core::arch::asm!("sti; hlt; pause");
    }
}

impl TaskManager {
    const fn uninit() -> Self {
        Self {
            core_cnt: 0,
            task_queue: SegQueue::new(),
            core_current_task: Vec::new(),
            processes: BTreeMap::new(),
        }
    }

    // Can only be called once
    pub unsafe fn init(&mut self, mapper: PageTableManager, core_cnt: u8) {
        self.core_cnt = core_cnt;
        let mut p = Process::new_with_page(mapper);
        assert!(p.pid == 0.into());

        for _ in 0..core_cnt {
            let t = unsafe { p.new_overide_thread() };
            self.core_current_task.push((p.pid, t));
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
        let current_cpu = get_current_cpu_id() as usize;
        let (pid, tid) = self.core_current_task[current_cpu];
        let thread = self.get_thread_mut(pid, tid)?;
        thread.save(stack_frame, reg);
        // Don't save nop task
        if pid != 0.into() {
            self.task_queue.push((pid, tid))
        }
        Some(())
    }

    fn load_new_task(&mut self, stack_frame: &mut InterruptStackFrame, reg: &mut Registers) {
        let current_cpu = get_current_cpu_id() as usize;
        // Loop becuase we don't delete tasks from queue when they exit
        loop {
            let (pid, tid) = self.get_next_task(current_cpu);
            if let Some((_p, thread)) = self.get_thread(pid, tid) {
                thread.restore(stack_frame, reg);
                // p.page_mapper.load_into_cr3();
                self.core_current_task[current_cpu] = (pid, tid);
                return;
            }
        }
    }

    pub fn switch_task(&mut self, stack_frame: &mut InterruptStackFrame, reg: &mut Registers) {
        self.save_current_task(stack_frame, reg);
        self.load_new_task(stack_frame, reg);
        // println!("{}", 1);
    }

    pub fn exit_thread(&mut self, stack_frame: &mut InterruptStackFrame, reg: &mut Registers) {
        let current_cpu = get_current_cpu_id() as usize;
        let (pid, tid) = self.core_current_task[current_cpu];
        let process = self.processes.get_mut(&pid).unwrap();
        process.threads.remove(&tid).unwrap();
        self.load_new_task(stack_frame, reg);
    }

    pub fn spawn_process(&mut self, _stack_frame: &mut InterruptStackFrame, reg: &mut Registers) {
        let mut process = Process::new();
        let pid = process.pid;

        // TODO: Validate r8 is a valid entrypoint
        let thread = process.new_thread(reg.r8);
        self.processes.insert(pid, process);
        self.task_queue.push((pid, thread));
        // Return process id as successful result;
        reg.rax = pid.into();
    }

    pub fn spawn_thread(&mut self, _stack_frame: &mut InterruptStackFrame, reg: &mut Registers) {
        let current_cpu = get_current_cpu_id() as usize;
        let (pid, _) = self.core_current_task[current_cpu];
        let process = self.processes.get_mut(&pid).unwrap();

        // TODO: Validate r8 is a valid entrypoint
        let thread = process.new_thread(reg.r8);
        self.task_queue.push((pid, thread));
        // Return task id as successful result;
        reg.rax = thread.into();
    }

    pub fn yield_now(&mut self, stack_frame: &mut InterruptStackFrame, reg: &mut Registers) {
        self.save_current_task(stack_frame, reg);
        self.load_new_task(stack_frame, reg);
    }
}

extern "C" {
    pub fn nop_task();
}
