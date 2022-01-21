use core::ptr::write_volatile;

use alloc::{
    boxed::Box,
    collections::{BTreeMap, VecDeque},
};
use spin::Mutex;
use x86_64::{
    instructions::hlt,
    structures::idt::{InterruptStackFrame, InterruptStackFrameValue},
    VirtAddr,
};

use crate::{assembly::registers::Registers, memory::get_uefi_active_mapper, syscall::exit};

use super::{Task, TaskID, TaskManager};

lazy_static! {
    static ref TASKS: Mutex<BTreeMap<TaskID, Task>> = Mutex::new(BTreeMap::new());
    static ref TASK_QUEUE: Mutex<VecDeque<TaskID>> = Mutex::new(VecDeque::new());
    static ref YIELDED_TASKS: Mutex<VecDeque<TaskID>> = Mutex::new(VecDeque::new());
    static ref NOP_TASK: Task = {
        let mut mapper = unsafe { get_uefi_active_mapper() };
        let mut t = Task::new(&mut mapper);
        t.state_isf.instruction_pointer = VirtAddr::from_ptr(nop_task as *const usize);
        t
    };
}

extern "C" fn nop_task() {
    loop {
        // Push all yielded tasks backinto the queue
        TASK_QUEUE.lock().append(&mut YIELDED_TASKS.lock());
        hlt();
    }
}

impl TaskManager {
    pub const fn new() -> Self {
        Self { current_task: None }
    }

    pub fn exit(&mut self, stack_frame: &mut InterruptStackFrame, regs: &mut Registers) {
        TASKS.lock().remove(&self.current_task.unwrap());
        // ! Maybe unmap and dealloc the memory for the stack?
        self.switch_to_next_task(stack_frame, regs);
    }

    pub fn yield_now(&mut self, stack_frame: &mut InterruptStackFrame, regs: &mut Registers) {
        if let Some(task) = self.current_task {
            TASKS.lock().get_mut(&task).unwrap().save(stack_frame, regs);
            YIELDED_TASKS.lock().push_back(self.current_task.unwrap());
        };

        self.switch_to_next_task(stack_frame, regs);
    }

    pub fn switch_task(&mut self, stack_frame: &mut InterruptStackFrame, regs: &mut Registers) {
        if let Some(task) = self.current_task {
            TASKS.lock().get_mut(&task).unwrap().save(stack_frame, regs);
            TASK_QUEUE.lock().push_back(task);
        };

        self.switch_to_next_task(stack_frame, regs);
    }

    fn switch_to_next_task(&mut self, stack_frame: &mut InterruptStackFrame, regs: &mut Registers) {
        if let Some(new_task) = TASK_QUEUE.lock().pop_front() {
            self.current_task = Some(new_task);

            unsafe { set_registers(stack_frame, regs, new_task) };
        } else {
            self.current_task = None;

            unsafe { set_nop_task(stack_frame, regs) };
        }
    }
}

unsafe fn set_registers(
    stack_frame: &mut InterruptStackFrame,
    regs: &mut Registers,
    task_id: TaskID,
) {
    let mut task_list = TASKS.lock();
    let task = task_list.get_mut(&task_id).unwrap();

    // Write the new tasks stack frame

    // TODO: Make this work again
    // stack_frame.as_mut().write(task.state_isf.clone());
    // Bad solution
    write_volatile(
        stack_frame.as_mut().extract_inner() as *mut InterruptStackFrameValue,
        task.state_isf.clone(),
    );
    // let sf = stack_frame.as_mut().extract_inner();
    // sf.instruction_pointer = task.state_isf.instruction_pointer;

    // Write the new tasks CPU registers
    write_volatile(regs, task.state_reg.clone());
}

/// Same as above
unsafe fn set_nop_task(stack_frame: &mut InterruptStackFrame, regs: &mut Registers) {
    write_volatile(
        stack_frame.as_mut().extract_inner() as *mut InterruptStackFrameValue,
        NOP_TASK.state_isf.clone(),
    );
    // let sf = stack_frame.as_mut().extract_inner();
    // sf.instruction_pointer = task.state_isf.instruction_pointer;

    // Write the new tasks CPU registers
    write_volatile(regs, NOP_TASK.state_reg.clone());
}

pub fn spawn(regs: &mut Registers) {
    let mut mapper = unsafe { get_uefi_active_mapper() };

    let mut task = Task::new(&mut mapper);

    // Return task id as successful result
    regs.rax = task.id.0;

    // Set startpoint to bootstraper
    task.state_isf.instruction_pointer = *THREAD_BOOTSTRAPER;

    // Pass function to first param
    task.state_reg.rdi = regs.r8;

    TASK_QUEUE.lock().push_back(task.id);
    if TASKS.lock().insert(task.id, task).is_some() {
        println!("Task with same ID already exists in tasks");
    }
}

use lazy_static::lazy_static;
lazy_static! {
    static ref THREAD_BOOTSTRAPER: VirtAddr = VirtAddr::from_ptr(thread_bootstraper as *mut usize);
}

extern "C" fn thread_bootstraper(main: usize) {
    // Recreate the function box that was passed from the syscall
    let func = unsafe { Box::from_raw(main as *mut Box<dyn FnOnce()>) };

    // Call the function
    func.call_once(());

    // Function ended quit
    exit()
}
