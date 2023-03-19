use core::ptr::{slice_from_raw_parts, slice_from_raw_parts_mut};

use alloc::{boxed::Box, sync::Arc};
use kernel_userspace::{
    stream::StreamMessage,
    syscall::{self, SYSCALL_NUMBER},
};
use x86_64::structures::idt::{InterruptDescriptorTable, InterruptStackFrame};

use crate::{
    assembly::registers::Registers,
    cpu_localstorage::get_task_mgr_current_pid,
    gdt::TASK_SWITCH_INDEX,
    paging::{
        get_uefi_active_mapper,
        page_allocator::request_page,
        page_table_manager::{Mapper, Page, Size4KB},
    },
    scheduling::{
        process::{PID, TID},
        taskmanager::TASKMANAGER,
    },
    stream::STREAMS,
    time::spin_sleep_ms,
    wrap_function_registers,
};

pub fn set_syscall_idt(idt: &mut InterruptDescriptorTable) {
    unsafe {
        idt[SYSCALL_NUMBER]
            .set_handler_fn(wrapped_syscall_handler)
            .set_stack_index(TASK_SWITCH_INDEX)
            .set_privilege_level(x86_64::PrivilegeLevel::Ring3);
    } // .disable_interrupts(false);
}

wrap_function_registers!(syscall_handler => wrapped_syscall_handler);

extern "C" fn syscall_handler(stack_frame: &mut InterruptStackFrame, regs: &mut Registers) {
    // Run syscalls without interrupts
    // This means execution should not be interrupted
    use kernel_userspace::syscall::*;
    match regs.rax {
        ECHO => echo_handler(regs),
        YIELD_NOW => {
            // Doesn't matter if it failed to lock, just give task more time
            TASKMANAGER
                .try_lock()
                .and_then(|mut t| Some(t.yield_now(stack_frame, regs)));
            // Ack interrupt
            unsafe { *(0xfee000b0 as *mut u32) = 0 }
            return;
        }
        SPAWN_PROCESS => {
            TASKMANAGER.lock().spawn_process(stack_frame, regs);
            return;
        }
        SPAWN_THREAD => {
            TASKMANAGER.lock().spawn_thread(stack_frame, regs);
        }
        // SLEEP => task_manager.sleep(stack_frame, regs),
        EXIT_THREAD => TASKMANAGER.lock().exit_thread(stack_frame, regs),
        MMAP_PAGE => mmap_page_handler(regs),
        STREAM => stream_handler(regs),
        READ_ARGS => read_args_handler(regs),
        _ => println!("Unknown syscall class: {}", regs.rax),
    }
    // Maybe give another task time
    TASKMANAGER
        .try_lock()
        .and_then(|mut t| Some(t.switch_task(stack_frame, regs)));

    // Ack interrupt
    unsafe { *(0xfee000b0 as *mut u32) = 0 }
}

unsafe fn syscall1(mut syscall_number: usize, arg1: usize) -> usize {
    core::arch::asm!("int 0x80", inout("rax") syscall_number, in("r8") arg1, options(nostack));
    syscall_number
}

unsafe fn syscall3(mut syscall_number: usize, arg1: usize, arg2: usize, arg3: usize) -> usize {
    core::arch::asm!("int 0x80", inout("rax") syscall_number, in("r8") arg1, in("r9") arg2, in("r10") arg3, options(nostack));
    syscall_number
}

/// Syscall test
/// Will return number passed as arg1
pub fn echo(number: usize) -> usize {
    unsafe { syscall1(syscall::ECHO, number) }
}

fn echo_handler(regs: &mut Registers) {
    println!("Echoing: {}", regs.r8);
    unsafe { core::arch::asm!("cli") }
    regs.rax = regs.r8
}

fn read_args_handler(regs: &mut Registers) {
    let pid = get_task_mgr_current_pid();
    let mut t = TASKMANAGER.lock();
    let proc = t.processes.get_mut(&pid).unwrap();

    if regs.r8 == 0 {
        regs.rax = proc.args.as_bytes().len();
    } else {
        let bytes = proc.args.as_bytes();
        let buf = unsafe { &mut *slice_from_raw_parts_mut(regs.r8 as *mut u8, bytes.len()) };
        buf.copy_from_slice(bytes);
    }
}

fn stream_handler(regs: &mut Registers) {
    let message = unsafe { &mut *(regs.r10 as *mut StreamMessage) };

    match regs.r8 {
        syscall::STREAM_CONNECT => {
            let nbytes = unsafe { &*slice_from_raw_parts(regs.r9 as *const u8, regs.r10) };
            let name = core::str::from_utf8(nbytes).unwrap();

            match STREAMS.lock().get_mut(name) {
                Some(st) => {
                    let pid = get_task_mgr_current_pid();
                    let mut t = TASKMANAGER.lock();
                    let process = t.processes.get_mut(&pid).unwrap();

                    process.streams.push(Arc::downgrade(&st));
                    regs.r8 = process.streams.len();
                    regs.rax = 0;
                }
                None => {
                    regs.rax = 1;
                }
            }
        }
        syscall::STREAM_PUSH => {
            match TASKMANAGER
                .lock()
                .get_stream(regs)
                .and_then(|s| s.upgrade())
                .and_then(|s| s.push(*message).ok())
            {
                Some(_) => regs.rax = 0,
                None => regs.rax = 1,
            }
        }
        syscall::STREAM_POP => {
            match TASKMANAGER
                .lock()
                .get_stream(regs)
                .and_then(|s| s.upgrade())
                .and_then(|s| s.pop())
            {
                Some(e) => {
                    *message = e;
                    regs.rax = 0
                }
                None => regs.rax = 1,
            };
        }
        _ => (),
    }
}

fn mmap_page_handler(regs: &mut Registers) {
    assert!(regs.r8 <= crate::paging::MemoryLoc::EndUserMem as usize);
    let page = request_page().unwrap();
    let mut mapper = unsafe { get_uefi_active_mapper() };
    mapper
        .map_memory(Page::<Size4KB>::new(regs.r8 as u64), Page::new(page))
        .unwrap()
        .flush();
}

pub fn yield_now() {
    unsafe { syscall1(syscall::YIELD_NOW, 0) };
    // unsafe { core::arch::asm!("hlt") }
}

pub fn spawn_process<F>(func: F, args: &str) -> PID
where
    F: Fn() + Send + Sync,
{
    let boxed_func: Box<dyn Fn()> = Box::new(func);
    let raw = Box::into_raw(Box::new(boxed_func)) as *mut usize;

    let res = unsafe {
        syscall3(
            syscall::SPAWN_PROCESS,
            raw as usize,
            args.as_ptr() as usize,
            args.len(),
        )
    } as u64;
    PID::from(res)
}

pub fn spawn_thread<F>(func: F) -> TID
where
    F: FnOnce() + Send + Sync,
{
    let boxed_func: Box<dyn FnOnce()> = Box::new(func);
    let raw = Box::into_raw(Box::new(boxed_func)) as *mut usize;
    let res = unsafe { syscall1(syscall::SPAWN_THREAD, raw as usize) } as u64;
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
    unsafe { syscall1(syscall::EXIT_THREAD, 0) };

    panic!("Function failed to QUIT")
}
