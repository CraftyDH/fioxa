use core::ptr::slice_from_raw_parts_mut;

use kernel_userspace::{
    service::{ReceiveMessageHeader, SendMessageHeader, SID},
    syscall::{self, yield_now, SYSCALL_NUMBER},
};
use x86_64::structures::idt::{InterruptDescriptorTable, InterruptStackFrame};

use crate::{
    assembly::registers::Registers,
    cpu_localstorage::{get_task_mgr_current_pid, get_task_mgr_current_tid},
    gdt::TASK_SWITCH_INDEX,
    paging::{
        page_allocator::request_page,
        page_table_manager::{Mapper, Page, Size4KB},
    },
    scheduling::taskmanager::{self, PROCESSES},
    service,
    time::{pit::get_uptime, spin_sleep_ms},
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
        YIELD_NOW => taskmanager::yield_now(stack_frame, regs),
        SPAWN_PROCESS => taskmanager::spawn_process(stack_frame, regs),
        SPAWN_THREAD => taskmanager::spawn_thread(stack_frame, regs),
        // SLEEP => task_manager.sleep(stack_frame, regs),
        EXIT_THREAD => taskmanager::exit_thread(stack_frame, regs),
        MMAP_PAGE => {
            mmap_page_handler(regs);
            taskmanager::yield_now(stack_frame, regs);
        }
        SERVICE => service_handler(regs),
        READ_ARGS => read_args_handler(regs),
        _ => println!("Unknown syscall class: {}", regs.rax),
    }

    // Ack interrupt
    unsafe { *(0xfee000b0 as *mut u32) = 0 }
}

fn echo_handler(regs: &mut Registers) {
    println!("Echoing: {}", regs.r8);
    unsafe { core::arch::asm!("cli") }
    regs.rax = regs.r8
}

fn read_args_handler(regs: &mut Registers) {
    let pid = get_task_mgr_current_pid();
    let p = PROCESSES.lock();
    let proc = p.get(&pid).unwrap();

    if regs.r8 == 0 {
        regs.rax = proc.args.len();
    } else {
        let bytes = &proc.args;
        let buf = unsafe { &mut *slice_from_raw_parts_mut(regs.r8 as *mut u8, bytes.len()) };
        buf.copy_from_slice(&bytes);
    }
}

fn service_handler(regs: &mut Registers) {
    match regs.r8 {
        syscall::SERVICE_CREATE => {
            let pid = get_task_mgr_current_pid();
            regs.rax = service::new(pid).0 as usize;
        }
        syscall::SERVICE_SUBSCRIBE => {
            let pid = get_task_mgr_current_pid();
            service::subscribe(pid, SID(regs.r9 as u64));
        }
        syscall::SERVICE_POP => {
            let pid = get_task_mgr_current_pid();
            let tid = get_task_mgr_current_tid();

            match service::pop(pid, tid, SID(regs.r10 as u64), regs.r11 as u64) {
                Some(msg) => {
                    unsafe {
                        core::ptr::copy_nonoverlapping(
                            &msg as *const ReceiveMessageHeader,
                            regs.r9 as *mut ReceiveMessageHeader,
                            1,
                        )
                    }
                    regs.rax = 0
                }
                None => regs.rax = 1,
            }
        }
        syscall::SERVICE_GETDATA => {
            let pid = get_task_mgr_current_pid();
            let tid = get_task_mgr_current_tid();

            match service::get_data(pid, tid, regs.r9 as *mut u8) {
                Some(_) => regs.rax = 0,
                None => regs.rax = 1,
            }
        }
        syscall::SERVICE_PUSH => {
            let pid = get_task_mgr_current_pid();

            let message: &SendMessageHeader = unsafe { &*(regs.r9 as *const SendMessageHeader) };

            match service::push(pid, message) {
                Some(_) => regs.rax = 0,
                None => regs.rax = 1,
            }
        }
        _ => (),
    }
}

fn mmap_page_handler(regs: &mut Registers) {
    assert!(regs.r8 <= crate::paging::MemoryLoc::EndUserMem as usize);

    let pid = get_task_mgr_current_pid();
    let mut processes = PROCESSES.lock();

    let process = processes.get_mut(&pid).unwrap();

    let page = Page::new(request_page().unwrap());

    process.owned_pages.push(page);

    process
        .page_mapper
        .map_memory(Page::<Size4KB>::new(regs.r8 as u64), page)
        .unwrap()
        .flush();
}

pub fn sleep(ms: usize) {
    // unsafe { syscall1(SLEEP, ms) };
    spin_sleep_ms(ms as u64)
}

pub fn syssleep(ms: u64) {
    let end = get_uptime() + ms as u64;
    while end > get_uptime() {
        yield_now()
    }
}
