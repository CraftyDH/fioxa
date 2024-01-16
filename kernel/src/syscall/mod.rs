use core::ptr::slice_from_raw_parts_mut;

use alloc::sync::Arc;
use kernel_userspace::{
    ids::ServiceID,
    service::ServiceTrackingNumber,
    syscall::{self, SYSCALL_NUMBER},
};
use x86_64::structures::idt::{InterruptDescriptorTable, InterruptStackFrame};

use crate::{
    assembly::registers::{Registers, SavedThreadState},
    cpu_localstorage::CPULocalStorageRW,
    gdt::TASK_SWITCH_INDEX,
    interrupts,
    paging::{
        page_allocator::frame_alloc_exec, page_mapper::PageMapping, page_table_manager::Mapper,
        MemoryMappingFlags,
    },
    scheduling::{
        process::ThreadContext,
        taskmanager::{self, kill_bad_task, load_new_task},
    },
    service,
    time::{self, pit, SLEEP_TARGET, SLEPT_PROCCESSES},
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

#[derive(Debug)]
pub enum SyscallError {
    OutofBoundsMem,
    MappingExists,
    OutOfMemory,
    UnMapError,
    KernelOnlySyscall,
}

extern "C" fn syscall_handler(stack_frame: &mut InterruptStackFrame, regs: &mut Registers) {
    // Run syscalls without interrupts
    // This means execution should not be interrupted
    use kernel_userspace::syscall::*;
    let res = match regs.rax {
        ECHO => echo_handler(regs),
        YIELD_NOW => Ok(taskmanager::yield_now(stack_frame, regs)),
        SPAWN_PROCESS => Ok(taskmanager::spawn_process(stack_frame, regs)),
        SPAWN_THREAD => Ok(taskmanager::spawn_thread(stack_frame, regs)),
        SLEEP => Ok(sleep_handler(stack_frame, regs)),
        EXIT_THREAD => Ok(taskmanager::exit_thread(stack_frame, regs)),
        MMAP_PAGE => mmap_page_handler(regs),
        MMAP_PAGE32 => mmap_page32_handler(regs),
        UNMMAP_PAGE => {
            // ! TODO: THIS IS VERY BAD
            // Another thread can still write to the memory
            unmmap_page_handler(regs)
        }
        SERVICE => service_handler(stack_frame, regs),
        READ_ARGS => read_args_handler(regs),
        GET_PID => {
            regs.rax = CPULocalStorageRW::get_current_pid().0 as usize;
            Ok(())
        }
        INTERNAL_KERNEL_WAKER => internal_kernel_waker_handler(stack_frame, regs),
        _ => {
            println!("Unknown syscall class: {}", regs.rax);
            Ok(())
        }
    };
    match res {
        Ok(()) => (),
        Err(e) => {
            println!("Error occored during syscall {e:?}");
            kill_bad_task()
        }
    }
}

pub fn check_mem_bounds(mem: usize) -> Result<usize, SyscallError> {
    if mem <= crate::paging::MemoryLoc::EndUserMem as usize {
        Ok(mem)
    } else {
        Err(SyscallError::OutofBoundsMem)
    }
}

fn echo_handler(regs: &mut Registers) -> Result<(), SyscallError> {
    println!("Echoing: {}", regs.r8);
    unsafe { core::arch::asm!("cli") }
    regs.rax = regs.r8;
    Ok(())
}

fn read_args_handler(regs: &mut Registers) -> Result<(), SyscallError> {
    let task = CPULocalStorageRW::get_current_task();

    let proc = &task.process;

    if regs.r8 == 0 {
        regs.rax = proc.args.len();
    } else {
        let bytes = &proc.args;
        let buf = unsafe { &mut *slice_from_raw_parts_mut(regs.r8 as *mut u8, bytes.len()) };
        buf.copy_from_slice(bytes);
    }
    Ok(())
}

fn service_handler(
    stack_frame: &mut InterruptStackFrame,
    regs: &mut Registers,
) -> Result<(), SyscallError> {
    match regs.r8 {
        syscall::SERVICE_CREATE => {
            let pid = CPULocalStorageRW::get_current_pid();
            regs.rax = service::new(pid).0 as usize;
        }
        syscall::SERVICE_SUBSCRIBE => {
            let pid = CPULocalStorageRW::get_current_pid();
            service::subscribe(pid, ServiceID(regs.r9 as u64));
        }
        syscall::SERVICE_PUSH => {
            let pid = CPULocalStorageRW::get_current_pid();

            let buf = unsafe { core::slice::from_raw_parts(regs.r9 as *const u8, regs.r10) };

            match service::push(pid, buf.into()) {
                Ok(_) => {
                    regs.rax = 0;
                }
                Err(e) => {
                    regs.rax = e.to_usize();
                }
            }
        }
        syscall::SERVICE_FETCH => {
            let thread = CPULocalStorageRW::get_current_task();

            match service::find_message(
                &thread,
                ServiceID(regs.r9 as u64),
                ServiceTrackingNumber(regs.r10 as u64),
            ) {
                Some(len) => regs.rax = len,
                None => regs.rax = 0,
            }
        }
        syscall::SERVICE_FETCH_WAIT => service::find_or_wait_message(
            stack_frame,
            regs,
            &CPULocalStorageRW::get_current_task(),
            ServiceID(regs.r9 as u64),
            ServiceTrackingNumber(regs.r10 as u64),
        ),
        syscall::SERVICE_GET => {
            let thread = CPULocalStorageRW::get_current_task();

            let buf = unsafe { core::slice::from_raw_parts_mut(regs.r9 as *mut u8, regs.r10) };
            match service::get_message(&thread, buf) {
                Some(_) => regs.rax = 0,
                None => regs.rax = 1,
            }
        }
        _ => (),
    }
    Ok(())
}

fn mmap_page_handler(regs: &mut Registers) -> Result<(), SyscallError> {
    check_mem_bounds(regs.r8)?;

    let task = CPULocalStorageRW::get_current_task();

    let mut memory = task.process.memory.lock();

    let lazy_page = PageMapping::new_lazy((regs.r9 + 0xFFF) & !0xFFF);

    if regs.r8 == 0 {
        regs.rax = memory
            .page_mapper
            .insert_mapping(lazy_page, MemoryMappingFlags::all());
    } else {
        memory
            .page_mapper
            .insert_mapping_at(regs.r8, lazy_page, MemoryMappingFlags::all())
            .ok_or(SyscallError::MappingExists)?;
        regs.rax = regs.r8;
    }
    Ok(())
}

fn mmap_page32_handler(regs: &mut Registers) -> Result<(), SyscallError> {
    let task = CPULocalStorageRW::get_current_task();

    let page =
        frame_alloc_exec(|m| m.request_32bit_reserved_page()).ok_or(SyscallError::OutOfMemory)?;

    regs.rax = page.get_address() as usize;

    let mut memory = task.process.memory.lock();
    unsafe {
        memory
            .page_mapper
            .get_mapper_mut()
            .identity_map_memory(*page, MemoryMappingFlags::all())
            .map_err(|_| SyscallError::MappingExists)?
            .flush();
    }
    memory.owned32_pages.push(page);
    Ok(())
}

fn unmmap_page_handler(regs: &Registers) -> Result<(), SyscallError> {
    check_mem_bounds(regs.r8)?;

    let task = CPULocalStorageRW::get_current_task();

    let mut memory = task.process.memory.lock();

    unsafe {
        memory
            .page_mapper
            .free_mapping(regs.r8..(regs.r8 + regs.r9 + 0xFFF) & !0xFFF)
            .map_err(|_| SyscallError::UnMapError)
    }
}

pub const INTERNAL_KERNEL_WAKER_INTERRUPTS: usize = 0;
pub const INTERNAL_KERNEL_WAKER_SLEEPD: usize = 1;

fn internal_kernel_waker_handler(
    stack_frame: &mut InterruptStackFrame,
    regs: &mut Registers,
) -> Result<(), SyscallError> {
    let thread = CPULocalStorageRW::get_current_task();

    match thread.process.privilege {
        crate::scheduling::process::ProcessPrivilige::KERNEL => (),
        crate::scheduling::process::ProcessPrivilige::USER => {
            return Err(SyscallError::KernelOnlySyscall);
        }
    };

    {
        let mut ctx = thread.context.lock();

        match core::mem::replace(&mut *ctx, ThreadContext::Invalid) {
            ThreadContext::Running(msg) => {
                *ctx = ThreadContext::Blocked(SavedThreadState::new(stack_frame, regs), msg)
            }
            e => panic!("thread was not running it was: {e:?}"),
        }
    }

    let res = match regs.r8 {
        INTERNAL_KERNEL_WAKER_INTERRUPTS => interrupts::DISPATCH_WAKER.set_waker(thread),
        INTERNAL_KERNEL_WAKER_SLEEPD => time::SLEEP_WAKER.set_waker(thread),
        _ => todo!(),
    };
    // Set successfully
    if res {
        taskmanager::load_new_task(stack_frame, regs);
    } else {
        // Race condition, it was set so return
        let thread = CPULocalStorageRW::get_current_task();
        let mut ctx = thread.context.lock();
        match core::mem::replace(&mut *ctx, ThreadContext::Invalid) {
            ThreadContext::Blocked(_, msg) => *ctx = ThreadContext::Running(msg),
            e => panic!("thread was not blocked it was: {e:?}"),
        }
    }
    Ok(())
}

fn sleep_handler(stack_frame: &mut InterruptStackFrame, regs: &mut Registers) {
    let time = pit::get_uptime() + regs.r8 as u64;
    let thread = CPULocalStorageRW::get_current_task();
    thread.context.lock().save(stack_frame, regs);

    SLEPT_PROCCESSES
        .lock()
        .insert(time, Arc::downgrade(&thread));

    // Ensure that the sleep waker is called if this was a shorter timeout
    let _ = SLEEP_TARGET.fetch_update(
        core::sync::atomic::Ordering::SeqCst,
        core::sync::atomic::Ordering::SeqCst,
        |val| Some(val.min(time)),
    );

    load_new_task(stack_frame, regs);
}
