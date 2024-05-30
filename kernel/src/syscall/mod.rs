use core::{fmt::Debug, num::NonZeroUsize, ptr::slice_from_raw_parts_mut, slice};

use alloc::{
    boxed::Box,
    string::{String, ToString},
    sync::Arc,
};
use kernel_userspace::{
    event::{
        EventCallback, EventQueueListenId, KernelEventQueueListenMode, KernelEventQueueOperation,
        ReceiveMode,
    },
    message::{MessageCreate, MessageGetSize, MessageRead, SyscallMessageAction},
    num_traits::FromPrimitive,
    object::{KernelReferenceID, ReferenceOperation},
    process::KernelProcessOperation,
    socket::{MakeSocket, SocketEvents, SocketOperation, SocketRecv},
    syscall::SYSCALL_NUMBER,
};
use spin::MutexGuard;
use x86_64::structures::idt::{InterruptDescriptorTable, InterruptStackFrame};

use crate::{
    assembly::registers::Registers,
    cpu_localstorage::CPULocalStorageRW,
    event::{EdgeTrigger, KEvent, KEventQueue},
    gdt::TASK_SWITCH_INDEX,
    message::KMessage,
    paging::{
        page_allocator::frame_alloc_exec, page_mapper::PageMapping, page_table_manager::Mapper,
        MemoryMappingFlags,
    },
    scheduling::{
        process::{KernelValue, ThreadStatus},
        taskmanager::{self, exit_thread_inner, kill_bad_task, load_new_task},
    },
    socket::{create_sockets, KSocketListener, PUBLIC_SOCKETS},
    time::{pit, SLEEP_TARGET, SLEPT_PROCESSES},
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
    Info(&'static str),
    Error,
}

trait Unwraper<T> {
    type Result;
    fn unwrap(self) -> Self::Result;
}

impl<T> Unwraper<T> for Option<T> {
    type Result = Result<T, Option<()>>;

    fn unwrap(self) -> Self::Result {
        self.ok_or(None)
    }
}

impl<T, U> Unwraper<T> for Result<T, U> {
    type Result = Result<T, U>;

    fn unwrap(self) -> Self::Result {
        self
    }
}

#[macro_export]
macro_rules! kpanic {
    ($($arg:tt)*) => {
        error!("Panicked in {}:{}:{} {}", file!(), line!(), column!(), format_args!($($arg)*));
        return Err(SyscallError::Error);
    };
}

#[macro_export]
macro_rules! kassert {
    ($x: expr) => {
        if !$x {
            error!("KAssert failed in {}:{}:{}.", file!(), line!(), column!());
            return Err(SyscallError::Error);
        }
    };
    ($x: expr, $($arg:tt)+) => {
        if !$x {
            error!("KAssert failed in {}:{}:{} {}", file!(), line!(), column!(), format_args!($($arg)*));
            return Err(SyscallError::Error);
        }
    };
}

#[macro_export]
macro_rules! kunwrap {
    ($x: expr) => {
        match Unwraper::unwrap($x) {
            Ok(r) => r,
            Err(e) => {
                error!(
                    "KUnwrap failed in {}:{}:{} on {e:?}",
                    file!(),
                    line!(),
                    column!()
                );
                return Err(SyscallError::Error);
            }
        }
    };
}

#[macro_export]
macro_rules! kenum_cast {
    ($x: expr, $t: path) => {
        match $x {
            $t(v) => v,
            _ => {
                error!(
                    "KEnum cast failed in {}:{}:{}, expected {} got {:?}.",
                    file!(),
                    line!(),
                    column!(),
                    stringify!($t),
                    $x
                );
                return Err(SyscallError::Error);
            }
        }
    };
}

unsafe extern "C" fn syscall_handler(stack_frame: &mut InterruptStackFrame, regs: &mut Registers) {
    // Run syscalls without interrupts
    // This means execution should not be interrupted
    use kernel_userspace::syscall::*;
    let res = match regs.rax {
        ECHO => echo_handler(regs),
        YIELD_NOW => Ok(taskmanager::yield_now(stack_frame, regs)),
        SPAWN_PROCESS => taskmanager::spawn_process(stack_frame, regs),
        SPAWN_THREAD => Ok(taskmanager::spawn_thread(stack_frame, regs)),
        SLEEP => sleep_handler(stack_frame, regs),
        EXIT_THREAD => Ok(taskmanager::exit_thread(stack_frame, regs)),
        MMAP_PAGE => mmap_page_handler(regs),
        MMAP_PAGE32 => mmap_page32_handler(regs),
        UNMMAP_PAGE => {
            // ! TODO: THIS IS VERY BAD
            // Another thread can still write to the memory
            unmmap_page_handler(regs)
        }
        READ_ARGS => read_args_handler(regs),
        GET_PID => {
            regs.rax = CPULocalStorageRW::get_current_pid().0 as usize;
            Ok(())
        }
        MESSAGE => message_handler(regs),
        EVENT => sys_receive_event(stack_frame, regs),
        EVENT_QUEUE => sys_event_queue(regs),
        SOCKET => sys_socket_handler(regs),
        OBJECT => sys_reference_handler(regs),
        PROCESS => sys_process_handler(regs),
        _ => {
            error!("Unknown syscall class: {}", regs.rax);
            Err(SyscallError::Error)
        }
    };
    match res {
        Ok(()) => (),
        Err(SyscallError::Error) => kill_bad_task(),
        Err(SyscallError::Info(e)) => {
            error!("Error occured during syscall {e:?}");
            kill_bad_task()
        }
    }
}

fn echo_handler(regs: &mut Registers) -> Result<(), SyscallError> {
    info!("Echoing: {}", regs.r8);
    regs.rax = regs.r8;
    Ok(())
}

unsafe fn read_args_handler(regs: &mut Registers) -> Result<(), SyscallError> {
    let task = CPULocalStorageRW::get_current_task();

    let proc = task.process();

    if regs.r8 == 0 {
        regs.rax = proc.args.len();
    } else {
        let bytes = &proc.args;
        let buf = unsafe { &mut *slice_from_raw_parts_mut(regs.r8 as *mut u8, bytes.len()) };
        buf.copy_from_slice(bytes);
    }
    Ok(())
}

unsafe fn mmap_page_handler(regs: &mut Registers) -> Result<(), SyscallError> {
    kassert!(regs.r8 <= crate::paging::MemoryLoc::EndUserMem as usize);

    let task = CPULocalStorageRW::get_current_task();

    let mut memory = task.process().memory.lock();

    let lazy_page = PageMapping::new_lazy((regs.r9 + 0xFFF) & !0xFFF);

    if regs.r8 == 0 {
        regs.rax = memory
            .page_mapper
            .insert_mapping(lazy_page, MemoryMappingFlags::all());
    } else {
        kunwrap!(memory.page_mapper.insert_mapping_at(
            regs.r8,
            lazy_page,
            MemoryMappingFlags::all()
        ));
        regs.rax = regs.r8;
    }
    Ok(())
}

unsafe fn mmap_page32_handler(regs: &mut Registers) -> Result<(), SyscallError> {
    let task = CPULocalStorageRW::get_current_task();

    let page = kunwrap!(frame_alloc_exec(|m| m.request_32bit_reserved_page()));

    regs.rax = page.get_address() as usize;

    let mut memory = task.process().memory.lock();
    unsafe {
        let map = kunwrap!(memory
            .page_mapper
            .get_mapper_mut()
            .identity_map_memory(*page, MemoryMappingFlags::all()));
        map.flush();
    }
    memory.owned32_pages.push(page);
    Ok(())
}

unsafe fn unmmap_page_handler(regs: &Registers) -> Result<(), SyscallError> {
    kassert!(regs.r8 <= crate::paging::MemoryLoc::EndUserMem as usize);

    let task = CPULocalStorageRW::get_current_task();

    let mut memory = task.process().memory.lock();

    unsafe {
        kunwrap!(memory
            .page_mapper
            .free_mapping(regs.r8..(regs.r8 + regs.r9 + 0xFFF) & !0xFFF));
        Ok(())
    }
}

unsafe fn sys_receive_event(
    stack_frame: &mut InterruptStackFrame,
    regs: &mut Registers,
) -> Result<(), SyscallError> {
    let event = kunwrap!(KernelReferenceID::from_usize(regs.r8));
    let thread = CPULocalStorageRW::get_current_task();

    let mode: ReceiveMode = kunwrap!(FromPrimitive::from_usize(regs.r9));

    let event = kunwrap!(thread.process().references.lock().references().get(&event)).clone();
    let event = kenum_cast!(event, KernelValue::Event);

    let ev = event.lock();

    let mut save_state = |mut event: MutexGuard<KEvent>, edge: EdgeTrigger| {
        let mut thread = CPULocalStorageRW::take_current_task();
        let handle = thread.handle().clone();
        let mut status = handle.status.lock();
        if let ThreadStatus::PleaseKill = *status {
            drop(event);
            drop(status);
            exit_thread_inner(thread);
        } else {
            thread.save_state(stack_frame, regs);
            thread.wait_on(&mut event, edge);
            *status = ThreadStatus::Blocked(thread);
            drop(event);
            drop(status);
        }

        taskmanager::load_new_task(stack_frame, regs);
        Ok(())
    };

    match mode {
        ReceiveMode::GetLevel => {
            regs.rax = ev.level() as usize;
            Ok(())
        }
        ReceiveMode::LevelHigh => {
            if ev.level() {
                regs.rax = 1;
                Ok(())
            } else {
                save_state(ev, EdgeTrigger::RISING_EDGE)
            }
        }
        ReceiveMode::LevelLow => {
            if !ev.level() {
                regs.rax = 0;
                Ok(())
            } else {
                save_state(ev, EdgeTrigger::FALLING_EDGE)
            }
        }
        ReceiveMode::Edge => save_state(ev, EdgeTrigger::RISING_EDGE | EdgeTrigger::FALLING_EDGE),
        ReceiveMode::EdgeHigh => save_state(ev, EdgeTrigger::RISING_EDGE),
        ReceiveMode::EdgeLow => save_state(ev, EdgeTrigger::FALLING_EDGE),
    }
}

unsafe fn sys_event_queue(regs: &mut Registers) -> Result<(), SyscallError> {
    let thread = CPULocalStorageRW::get_current_task();

    let operation: KernelEventQueueOperation = kunwrap!(FromPrimitive::from_usize(regs.r8));

    let get_event = || {
        let id = kunwrap!(KernelReferenceID::from_usize(regs.r9));
        let event = kunwrap!(thread.process().get_value(id));
        let event = kenum_cast!(event, KernelValue::EventQueue);
        Ok(event)
    };

    match operation {
        KernelEventQueueOperation::Create => {
            let new = KEventQueue::new();
            let id = thread.process().add_value(new.into());
            regs.rax = id.0.get();
        }
        KernelEventQueueOperation::GetEvent => {
            let ev = get_event()?;
            let event = ev.event();
            let id = thread.process().add_value(event.into());
            regs.rax = id.0.get();
        }
        KernelEventQueueOperation::PopQueue => {
            let ev = get_event()?;
            match ev.try_pop_event() {
                Some(e) => regs.rax = e.0.get(),
                None => regs.rax = 0,
            }
        }
        KernelEventQueueOperation::Listen => {
            let ev = get_event()?;

            let listen_id = kunwrap!(KernelReferenceID::from_usize(regs.r10));
            let callback = kunwrap!(NonZeroUsize::new(regs.r11).map(EventCallback));

            let listen_event = kunwrap!(thread.process().get_value(listen_id));
            let listen_event = kenum_cast!(listen_event, KernelValue::Event);
            let mode: KernelEventQueueListenMode = kunwrap!(FromPrimitive::from_usize(regs.r12));

            regs.rax = ev.listen(listen_event, callback, mode)?.0.get();
        }
        KernelEventQueueOperation::Unlisten => {
            let ev = get_event()?;
            let listen_id = EventQueueListenId(kunwrap!(NonZeroUsize::new(regs.r10)));
            kunwrap!(ev.unlisten(listen_id));
        }
    }

    Ok(())
}

unsafe fn sys_socket_handler(regs: &mut Registers) -> Result<(), SyscallError> {
    let thread = CPULocalStorageRW::get_current_task();

    let operation: SocketOperation = kunwrap!(FromPrimitive::from_usize(regs.r8));

    match operation {
        SocketOperation::Listen => {
            let name = slice::from_raw_parts(regs.r9 as *const u8, regs.r10);
            let name = String::from_utf8_lossy(name).to_string();
            match PUBLIC_SOCKETS.lock().entry(name) {
                hashbrown::hash_map::Entry::Occupied(_) => regs.rax = 0,
                hashbrown::hash_map::Entry::Vacant(place) => {
                    let handle = KSocketListener::new();
                    place.insert(handle.clone());
                    regs.rax = thread.process().add_value(handle.into()).0.get();
                }
            }
        }
        SocketOperation::Connect => {
            let name = slice::from_raw_parts(regs.r9 as *const u8, regs.r10);
            let name = kunwrap!(core::str::from_utf8(name));
            match PUBLIC_SOCKETS.lock().get(name) {
                Some(listener) => {
                    regs.rax = thread
                        .process()
                        .add_value(listener.connect().into())
                        .0
                        .get();
                }
                None => regs.rax = 0,
            }
        }
        SocketOperation::Accept => {
            let id = kunwrap!(KernelReferenceID::from_usize(regs.r9));

            let sock = kunwrap!(thread.process().get_value(id));
            let sock = kenum_cast!(sock, KernelValue::SocketListener);

            match sock.pop() {
                Some(val) => {
                    regs.rax = thread.process().add_value(val.into()).0.get();
                }
                None => regs.rax = 0,
            }
        }
        SocketOperation::GetSocketListenEvent => {
            let id = kunwrap!(KernelReferenceID::from_usize(regs.r9));

            let sock = kunwrap!(thread.process().get_value(id));
            let sock = kenum_cast!(sock, KernelValue::SocketListener);

            regs.rax = thread.process().add_value(sock.event().into()).0.get();
        }
        SocketOperation::Create => {
            let info = &mut *(regs.r9 as *mut MakeSocket);
            let sockets = create_sockets(info.ltr_capacity, info.rtl_capacity);
            let refs = &mut thread.process().references.lock();
            info.left.write(refs.add_value(sockets.0.into()));
            info.right.write(refs.add_value(sockets.1.into()));
            regs.rax = 0;
        }
        SocketOperation::GetSocketEvent => {
            let id = kunwrap!(KernelReferenceID::from_usize(regs.r9));
            let ev: SocketEvents = kunwrap!(FromPrimitive::from_usize(regs.r10));

            let sock = kunwrap!(thread.process().get_value(id));
            let sock = kenum_cast!(sock, KernelValue::Socket);

            let event = sock.get_event(ev);

            let id = thread.process().add_value(event.into());
            regs.rax = id.0.get();
        }
        SocketOperation::Send => {
            let id = kunwrap!(KernelReferenceID::from_usize(regs.r9));
            let msgid = kunwrap!(KernelReferenceID::from_usize(regs.r10));

            let sock = kunwrap!(thread.process().get_value(id));
            let message = kunwrap!(thread.process().get_value(msgid));

            let sock = kenum_cast!(sock, KernelValue::Socket);
            regs.rax = match sock.send_message(message) {
                Some(()) => 0,
                None if sock.is_eof() => 2,
                None => 1,
            }
        }
        SocketOperation::Recv => unsafe {
            let recv = &mut *(regs.r9 as *mut SocketRecv);

            let sock = kunwrap!(thread.process().get_value(recv.socket));

            let sock = kenum_cast!(sock, KernelValue::Socket);

            match sock.recv_message() {
                Some(message) => {
                    recv.result_type.write(message.object_type());
                    recv.result = Some(thread.process().add_value(message));
                }
                None => recv.eof = sock.is_eof(),
            }
        },
    }

    Ok(())
}

unsafe fn sys_reference_handler(regs: &mut Registers) -> Result<(), SyscallError> {
    let thread = CPULocalStorageRW::get_current_task();

    let operation: ReferenceOperation = kunwrap!(FromPrimitive::from_usize(regs.r8));
    let id = kunwrap!(KernelReferenceID::from_usize(regs.r9));

    let mut refs = thread.process().references.lock();
    match operation {
        ReferenceOperation::Clone => {
            let clonable = kunwrap!(refs.references().get(&id)).clone();
            regs.rax = refs.add_value(clonable).0.get();
        }
        ReferenceOperation::Delete => {
            kunwrap!(refs.references().remove(&id));
        }
        ReferenceOperation::GetType => {
            regs.rax = match refs.references().get(&id) {
                Some(r) => r.object_type(),
                None => kernel_userspace::object::KernelObjectType::None,
            } as usize;
        }
    }
    Ok(())
}

unsafe fn sys_process_handler(regs: &mut Registers) -> Result<(), SyscallError> {
    let thread = CPULocalStorageRW::get_current_task();

    let operation: KernelProcessOperation = kunwrap!(FromPrimitive::from_usize(regs.r8));
    let id = kunwrap!(KernelReferenceID::from_usize(regs.r9));
    let proc = kunwrap!(thread.process().get_value(id));

    let proc = kenum_cast!(proc, KernelValue::Process);

    match operation {
        KernelProcessOperation::GetExitCode => {
            regs.rax = *proc.exit_status.lock() as usize;
        }
        KernelProcessOperation::GetExitEvent => {
            regs.rax = thread
                .process()
                .add_value(proc.exit_signal.clone().into())
                .0
                .get();
        }
        KernelProcessOperation::Kill => {
            proc.kill_threads();
        }
    }
    Ok(())
}

unsafe fn sleep_handler(
    stack_frame: &mut InterruptStackFrame,
    regs: &mut Registers,
) -> Result<(), SyscallError> {
    let time = pit::get_uptime() + regs.r8 as u64;
    let mut thread = CPULocalStorageRW::take_current_task();
    thread.save_state(stack_frame, regs);

    let handle = thread.handle().clone();
    let mut status = handle.status.lock();
    if let ThreadStatus::PleaseKill = *status {
        drop(status);
        exit_thread_inner(thread);
        load_new_task(stack_frame, regs);
        return Ok(());
    }
    *status = ThreadStatus::Blocked(thread);
    drop(status);

    SLEPT_PROCESSES
        .lock()
        .entry(time)
        .or_default()
        .push(Arc::downgrade(&handle));

    // Ensure that the sleep waker is called if this was a shorter timeout
    let _ = SLEEP_TARGET.fetch_update(
        core::sync::atomic::Ordering::SeqCst,
        core::sync::atomic::Ordering::SeqCst,
        |val| Some(val.min(time)),
    );

    load_new_task(stack_frame, regs);
    Ok(())
}

unsafe fn message_handler(regs: &mut Registers) -> Result<(), SyscallError> {
    let action: SyscallMessageAction = kunwrap!(FromPrimitive::from_usize(regs.r8));
    let thread = CPULocalStorageRW::get_current_task();

    match action {
        SyscallMessageAction::Create => unsafe {
            let msg_create = &mut *(regs.r9 as *mut MessageCreate);
            let req = &msg_create.before;
            let data: Box<[u8]> = core::slice::from_raw_parts(req.0, req.1).into();
            let msg = Arc::new(KMessage { data });

            msg_create.after = thread.process().add_value(msg.into());
        },
        SyscallMessageAction::GetSize => unsafe {
            let msg_size = &mut *(regs.r9 as *mut MessageGetSize);

            let msg = kunwrap!(thread.process().get_value(msg_size.before));
            let msg = kenum_cast!(msg, KernelValue::Message);

            msg_size.after = msg.data.len();
        },
        SyscallMessageAction::Read => unsafe {
            let msg_read = &mut *(regs.r9 as *mut MessageRead);

            let loc = core::slice::from_raw_parts_mut(msg_read.ptr.0, msg_read.ptr.1);

            let msg = kunwrap!(thread.process().get_value(msg_read.id));
            let msg = kenum_cast!(msg, KernelValue::Message);

            let data = &msg.data;

            kassert!(
                data.len() == loc.len(),
                "Data and loc len should be same instead was: {} {}",
                data.len(),
                loc.len()
            );

            loc.copy_from_slice(data);
        },
    }

    Ok(())
}
