use core::ptr::slice_from_raw_parts_mut;

use kernel_userspace::{
    ids::ServiceID,
    service::ServiceTrackingNumber,
    syscall::{self, yield_now, SYSCALL_NUMBER},
};
use x86_64::structures::idt::{InterruptDescriptorTable, InterruptStackFrame};

use crate::{
    assembly::registers::Registers,
    cpu_localstorage::CPULocalStorageRW,
    gdt::TASK_SWITCH_INDEX,
    paging::{
        page_allocator::frame_alloc_exec, page_mapper::PageMapping, page_table_manager::Mapper,
    },
    scheduling::taskmanager::{self, kill_bad_task},
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

#[derive(Debug)]
pub enum SyscallError {
    OutofBoundsMem,
    MappingExists,
    OutOfMemory,
    UnMapError,
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
        // SLEEP => task_manager.sleep(stack_frame, regs),
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
        regs.rax = memory.page_mapper.insert_mapping(lazy_page);
    } else {
        memory
            .page_mapper
            .insert_mapping_at(regs.r8, lazy_page)
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
            .identity_map_memory(*page)
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
            .free_mapping(regs.r8..regs.r8 + (regs.r9 + 0xFFF) & !0xFFF)
            .map_err(|_| SyscallError::UnMapError)
    }
}

pub fn sleep(ms: usize) {
    // unsafe { syscall1(SLEEP, ms) };
    spin_sleep_ms(ms as u64)
}

pub fn syssleep(ms: u64) {
    let end = get_uptime() + ms;
    while end > get_uptime() {
        yield_now()
    }
}
