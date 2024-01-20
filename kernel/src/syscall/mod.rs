use core::ptr::slice_from_raw_parts_mut;

use alloc::{boxed::Box, sync::Arc};
use kernel_userspace::{
    ids::ServiceID,
    message::{
        MessageClone, MessageCreate, MessageDrop, MessageGetSize, MessageRead, SyscallMessageAction,
    },
    num_traits::FromPrimitive,
    service::{SendError, ServiceMessageK, ServiceTrackingNumber},
    syscall::{self, SYSCALL_NUMBER},
};
use x86_64::structures::idt::{InterruptDescriptorTable, InterruptStackFrame};

use crate::{
    assembly::registers::{Registers, SavedThreadState},
    cpu_localstorage::CPULocalStorageRW,
    gdt::TASK_SWITCH_INDEX,
    interrupts,
    message::{create_new_messageid, KMessage, KMessageProcRefcount},
    paging::{
        page_allocator::frame_alloc_exec, page_mapper::PageMapping, page_table_manager::Mapper,
        MemoryMappingFlags,
    },
    scheduling::{
        process::ThreadContext,
        taskmanager::{self, kill_bad_task, load_new_task},
    },
    service::{self, service_wait},
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
    UnknownSubAction,
    MessageIdUnknown,
    MessageReadWrongSize,
    SendError(SendError),
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
        MESSAGE => message_handler(regs),
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

            let msg = unsafe { &*(regs.r9 as *const ServiceMessageK) };

            service::push(pid, msg).map_err(|e| SyscallError::SendError(e))?;
        }
        syscall::SERVICE_FETCH => {
            let thread = CPULocalStorageRW::get_current_task();

            match service::try_find_message(
                &thread,
                ServiceID(regs.r10 as u64),
                ServiceTrackingNumber(regs.r11 as u64),
            ) {
                Some(len) => unsafe {
                    *(regs.r9 as *mut ServiceMessageK) = len;
                    regs.rax = 1
                },
                None => regs.rax = 0,
            }
        }
        syscall::SERVICE_WAIT => {
            let thread = CPULocalStorageRW::get_current_task();

            {
                let mut ctx = thread.context.lock();

                match core::mem::replace(&mut *ctx, ThreadContext::Invalid) {
                    ThreadContext::Running => {
                        *ctx = ThreadContext::Blocked(SavedThreadState::new(stack_frame, regs))
                    }
                    e => panic!("thread was not running it was: {e:?}"),
                }
            }

            let res = service_wait(thread, ServiceID(regs.r9 as u64));
            // Set successfully
            if res {
                taskmanager::load_new_task(stack_frame, regs);
            } else {
                // Race condition, it was set so return
                let thread = CPULocalStorageRW::get_current_task();
                let mut ctx = thread.context.lock();
                match core::mem::replace(&mut *ctx, ThreadContext::Invalid) {
                    ThreadContext::Blocked(_) => *ctx = ThreadContext::Running,
                    e => panic!("thread was not blocked it was: {e:?}"),
                }
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
            ThreadContext::Running => {
                *ctx = ThreadContext::Blocked(SavedThreadState::new(stack_frame, regs))
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
            ThreadContext::Blocked(_) => *ctx = ThreadContext::Running,
            e => panic!("thread was not blocked it was: {e:?}"),
        }
    }
    Ok(())
}

fn sleep_handler(stack_frame: &mut InterruptStackFrame, regs: &mut Registers) {
    let time = pit::get_uptime() + regs.r8 as u64;
    let thread = CPULocalStorageRW::get_current_task();
    thread.context.lock().save(stack_frame, regs);

    // println!("Sleep {} {}", thread.process.pid.0, thread.tid.0);

    SLEPT_PROCCESSES
        .lock()
        .entry(time)
        .or_default()
        .push(Arc::downgrade(&thread));

    // Ensure that the sleep waker is called if this was a shorter timeout
    let _ = SLEEP_TARGET.fetch_update(
        core::sync::atomic::Ordering::SeqCst,
        core::sync::atomic::Ordering::SeqCst,
        |val| Some(val.min(time)),
    );

    load_new_task(stack_frame, regs);
}

fn message_handler(regs: &mut Registers) -> Result<(), SyscallError> {
    let action: SyscallMessageAction =
        FromPrimitive::from_usize(regs.r8).ok_or(SyscallError::UnknownSubAction)?;
    let thread = CPULocalStorageRW::get_current_task();

    match action {
        SyscallMessageAction::Create => unsafe {
            let msg_create = &mut *(regs.r9 as *mut MessageCreate);
            let req = &msg_create.before;
            let data: Box<[u8]> = core::slice::from_raw_parts(req.0, req.1).into();
            let id = create_new_messageid();
            let msg = Arc::new(KMessage { id, data });
            assert!(thread
                .process
                .service_messages
                .lock()
                .messages
                .insert(id, KMessageProcRefcount { msg, ref_count: 1 })
                .is_none());
            msg_create.after = id;
        },
        SyscallMessageAction::GetSize => unsafe {
            let msg_size = &mut *(regs.r9 as *mut MessageGetSize);
            msg_size.after = thread
                .process
                .service_messages
                .lock()
                .messages
                .get(&msg_size.before)
                .ok_or(SyscallError::MessageIdUnknown)?
                .msg
                .data
                .len();
        },
        SyscallMessageAction::Read => unsafe {
            let msg_read = &mut *(regs.r9 as *mut MessageRead);

            let loc = core::slice::from_raw_parts_mut(msg_read.ptr.0, msg_read.ptr.1);

            let messages = thread.process.service_messages.lock();
            let msg = messages
                .messages
                .get(&msg_read.id)
                .ok_or(SyscallError::MessageIdUnknown)?;

            let data = &msg.msg.data;
            if data.len() != loc.len() {
                return Err(SyscallError::MessageReadWrongSize);
            }

            loc.copy_from_slice(&data);
        },
        SyscallMessageAction::Clone => unsafe {
            let msg_clone = &mut *(regs.r9 as *mut MessageClone);

            thread
                .process
                .service_messages
                .lock()
                .messages
                .get_mut(&msg_clone.0)
                .ok_or(SyscallError::MessageIdUnknown)?
                .ref_count += 1;
        },
        SyscallMessageAction::Drop => unsafe {
            let msg_drop = &mut *(regs.r9 as *mut MessageDrop);

            let mut messages = thread.process.service_messages.lock();

            let cnt = {
                let msg = messages
                    .messages
                    .get_mut(&msg_drop.0)
                    .ok_or(SyscallError::MessageIdUnknown)?;
                let cnt = msg.ref_count.saturating_sub(1);
                msg.ref_count = cnt;
                cnt
            };
            if cnt == 0 {
                messages
                    .messages
                    .remove(&msg_drop.0)
                    .expect("the entry should be in the map");
            }
        },
    }

    Ok(())
}
