use alloc::boxed::Box;
use x86_64::structures::idt::{InterruptDescriptorTable, InterruptStackFrame};

use crate::{
    assembly::registers::Registers,
    gdt::TASK_SWITCH_INDEX,
    scheduling::{
        process::{PID, TID},
        taskmanager::TASKMANAGER,
    },
    time::spin_sleep_ms,
    wrap_function_registers,
};

pub const SYSCALL_ADDR: usize = 0x80;
const ECHO: usize = 0;
const YIELD_NOW: usize = 1;
const SPAWN_PROCESS: usize = 2;
const SPAWN_THREAD: usize = 3;
const SLEEP: usize = 4;
const EXIT_THREAD: usize = 5;

pub fn set_syscall_idt(idt: &mut InterruptDescriptorTable) {
    unsafe {
        idt[SYSCALL_ADDR]
            .set_handler_fn(wrapped_syscall_handler)
            .set_stack_index(TASK_SWITCH_INDEX)
            .set_privilege_level(x86_64::PrivilegeLevel::Ring3);
    } // .disable_interrupts(false);
}

wrap_function_registers!(syscall_handler => wrapped_syscall_handler);

extern "C" fn syscall_handler(stack_frame: &mut InterruptStackFrame, regs: &mut Registers) {
    // Run syscalls without interrupts
    // This means execution should not be interrupted
    match regs.rax {
        ECHO => echo_handler(regs),
        YIELD_NOW => {
            // Doesn't matter if it failed to lock, just give task more time
            TASKMANAGER
                .try_lock()
                .and_then(|mut t| Some(t.yield_now(stack_frame, regs)));
        }
        SPAWN_PROCESS => TASKMANAGER.lock().spawn_process(stack_frame, regs),
        SPAWN_THREAD => TASKMANAGER.lock().spawn_thread(stack_frame, regs),
        // SLEEP => task_manager.sleep(stack_frame, regs),
        EXIT_THREAD => TASKMANAGER.lock().exit_thread(stack_frame, regs),
        _ => println!("Unknown syscall class: {}", regs.rax),
    }
    // Ack interrupt
    unsafe { *(0xfee000b0 as *mut u32) = 0 }
}

unsafe fn syscall1(mut syscall_number: usize, arg1: usize) -> usize {
    core::arch::asm!("int 0x80", inout("rax") syscall_number, in("r8") arg1, options(nostack));
    syscall_number
}

/// Syscall test
/// Will return number passed as arg1
pub fn echo(number: usize) -> usize {
    unsafe { syscall1(ECHO, number) }
}

fn echo_handler(regs: &mut Registers) {
    println!("Echoing: {}", regs.r8);
    unsafe { core::arch::asm!("cli") }
    regs.rax = regs.r8
}

pub fn yield_now() {
    unsafe { syscall1(YIELD_NOW, 0) };
    // unsafe { core::arch::asm!("hlt") }
}

pub fn spawn_process<F>(func: F) -> PID
where
    F: Fn() + Send + Sync,
{
    let boxed_func: Box<dyn Fn()> = Box::new(func);
    let raw = Box::into_raw(Box::new(boxed_func)) as *mut usize;
    let res = unsafe { syscall1(SPAWN_PROCESS, raw as usize) } as u64;
    PID::from(res)
}

pub fn spawn_thread<F>(func: F) -> TID
where
    F: FnOnce() + Send + Sync,
{
    let boxed_func: Box<dyn FnOnce()> = Box::new(func);
    let raw = Box::into_raw(Box::new(boxed_func)) as *mut usize;
    let res = unsafe { syscall1(SPAWN_THREAD, raw as usize) } as u64;
    TID::from(res)
}

pub fn sleep(ms: usize) {
    // unsafe { syscall1(SLEEP, ms) };
    spin_sleep_ms(ms as u64)
    // let end = get_uptime() + ms;
    // while end > get_uptime() {
    //     unsafe { _mm_pause() };
    // }
}

pub fn exit_thread() -> ! {
    unsafe { syscall1(EXIT_THREAD, 0) };

    panic!("Function failed to QUIT")
}
