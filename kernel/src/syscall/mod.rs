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
    cpu_localstorage::CPULocalStorageRW,
    event::{EdgeTrigger, KEvent, KEventQueue},
    message::KMessage,
    paging::{
        page_allocator::frame_alloc_exec, page_mapper::PageMapping, page_table_manager::Mapper,
        MemoryMappingFlags,
    },
    scheduling::{
        process::KernelValue,
        taskmanager::{self, block_task, exit_task, kill_bad_task, yield_task},
    },
    socket::{create_sockets, KSocketListener, PUBLIC_SOCKETS},
    time::{uptime, SLEPT_PROCESSES},
};

pub fn set_syscall_idt(idt: &mut InterruptDescriptorTable) {
    idt[SYSCALL_NUMBER]
        .set_handler_fn(wrapped_syscall_handler)
        .set_privilege_level(x86_64::PrivilegeLevel::Ring3);
    // .disable_interrupts(false);
}

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

/// Handler for internal syscalls called by the kernel (Note: not nested).
#[naked]
pub unsafe extern "C" fn syscall_kernel_handler() {
    core::arch::asm!(
        "push rbp",
        "push r15",
        "pushfq",
        "cli",
        "mov al, 1",
        "mov gs:0x9, al",
        "mov r15, rsp",     // save caller rsp
        "mov rsp, gs:0x1A", // load kstack top
        "sti",
        "call {}",
        "cli",
        "mov cl, 2",
        "mov gs:0x9, cl", // set cpu context
        "mov rsp, r15",   // restore caller rip
        "popfq",
        "pop r15",
        "pop rbp",
        "ret",
        sym syscall_handler,
        options(noreturn)
    );
}

/// Handler for syscalls via int 0x80
#[naked]
pub extern "x86-interrupt" fn wrapped_syscall_handler(_: InterruptStackFrame) {
    unsafe {
        core::arch::asm!(
            "mov al, 1",
            "mov gs:0x9, al", // set cpu context
            "sti",
            "call {}",
            "cli",
            "mov cl, 2",
            "mov gs:0x9, cl", // set cpu context
            // clear scratch registers (we don't want leaks)
            "xor r11d, r11d",
            "xor r10d, r10d",
            "xor r9d,  r9d",
            "xor r8d,  r8d",
            "xor edi,  edi",
            "xor esi,  esi",
            "xor edx,  edx",
            "xor ecx,  ecx",
            "iretq",
            sym syscall_handler,
            options(noreturn)
        );
    }
}

/// Handler for syscalls via syscall
#[naked]
pub unsafe extern "C" fn syscall_sysret_handler() {
    core::arch::asm!(
        "mov al, 1",
        "mov gs:0x9, al",
        "mov r15, rsp", // save caller rsp
        "mov r14, r11", // save caller flags
        "mov r13, rcx", // save caller rip
        "mov rcx, r10", // move arg3 to match sysv c calling convention
        "mov rsp, gs:0x1A", // load kstack top
        "sti",
        "call {}",
        // clear scratch registers (we don't want leaks)
        "cli",
        "xor r10d, r10d",
        "xor r9d,  r9d",
        "xor r8d,  r8d",
        "xor edi,  edi",
        "xor esi,  esi",
        "mov cl, 2",
        "mov gs:0x9, cl", // set cpu context
        "mov rcx, r13", // restore caller rip
        "mov r11, r14", // restore caller flags
        "mov rsp, r15", // restore caller rsp
        "sysretq",
        sym syscall_handler,
        options(noreturn)
    );
}

unsafe extern "C" fn syscall_handler(
    number: usize,
    arg1: usize,
    arg2: usize,
    arg3: usize,
    arg4: usize,
    arg5: usize,
) -> usize {
    // Run syscalls without interrupts
    // This means execution should not be interrupted
    let thread = CPULocalStorageRW::get_current_task();
    assert!(!thread.in_syscall);
    thread.in_syscall = true;
    use kernel_userspace::syscall::*;
    let res = match number {
        ECHO => echo_handler(arg1),
        YIELD_NOW => {
            yield_task();
            Ok(0)
        }
        SPAWN_PROCESS => taskmanager::spawn_process(arg1, arg2, arg3, arg4),
        SPAWN_THREAD => taskmanager::spawn_thread(arg1, arg2),
        SLEEP => sleep_handler(arg1),
        EXIT_THREAD => exit_task(),
        MMAP_PAGE => mmap_page_handler(arg1, arg2),
        MMAP_PAGE32 => mmap_page32_handler(),
        UNMMAP_PAGE => {
            // ! TODO: THIS IS VERY BAD
            // Another thread can still write to the memory
            unmmap_page_handler(arg1, arg2)
        }
        READ_ARGS => read_args_handler(arg1),
        GET_PID => Ok(thread.process().pid.0 as usize),
        MESSAGE => message_handler(arg1, arg2),
        EVENT => sys_receive_event(arg1, arg2),
        EVENT_QUEUE => sys_event_queue(arg1, arg2, arg3, arg4, arg5),
        SOCKET => sys_socket_handler(arg1, arg2, arg3),
        OBJECT => sys_reference_handler(arg1, arg2),
        PROCESS => sys_process_handler(arg1, arg2),
        _ => {
            error!("Unknown syscall class: {}", number);
            Err(SyscallError::Error)
        }
    };
    thread.in_syscall = false;
    match res {
        Ok(r) => r,
        Err(SyscallError::Error) => kill_bad_task(),
        Err(SyscallError::Info(e)) => {
            error!("Error occured during syscall {e:?}");
            kill_bad_task()
        }
    }
}

fn echo_handler(arg1: usize) -> Result<usize, SyscallError> {
    info!("Echoing: {}", arg1);
    Ok(arg1)
}

unsafe fn read_args_handler(arg1: usize) -> Result<usize, SyscallError> {
    let task = CPULocalStorageRW::get_current_task();

    let proc = task.process();

    if arg1 == 0 {
        Ok(proc.args.len())
    } else {
        let bytes = &proc.args;
        let buf = unsafe { &mut *slice_from_raw_parts_mut(arg1 as *mut u8, bytes.len()) };
        buf.copy_from_slice(bytes);
        Ok(arg1)
    }
}

unsafe fn mmap_page_handler(arg1: usize, arg2: usize) -> Result<usize, SyscallError> {
    kassert!(arg1 <= crate::paging::MemoryLoc::EndUserMem as usize);

    let task = CPULocalStorageRW::get_current_task();

    let mut memory = task.process().memory.lock();

    let lazy_page = PageMapping::new_lazy((arg2 + 0xFFF) & !0xFFF);

    if arg1 == 0 {
        Ok(memory
            .page_mapper
            .insert_mapping(lazy_page, MemoryMappingFlags::all()))
    } else {
        kunwrap!(memory
            .page_mapper
            .insert_mapping_at(arg1, lazy_page, MemoryMappingFlags::all()));
        Ok(arg1)
    }
}

unsafe fn mmap_page32_handler() -> Result<usize, SyscallError> {
    let task = CPULocalStorageRW::get_current_task();

    let page = kunwrap!(frame_alloc_exec(|m| m.request_32bit_reserved_page()));

    let r = page.get_address() as usize;

    let mut memory = task.process().memory.lock();
    unsafe {
        let map = kunwrap!(memory
            .page_mapper
            .get_mapper_mut()
            .identity_map_memory(*page, MemoryMappingFlags::all()));
        map.flush();
    }
    memory.owned32_pages.push(page);
    Ok(r)
}

unsafe fn unmmap_page_handler(arg1: usize, arg2: usize) -> Result<usize, SyscallError> {
    kassert!(arg1 <= crate::paging::MemoryLoc::EndUserMem as usize);

    let task = CPULocalStorageRW::get_current_task();

    let mut memory = task.process().memory.lock();

    unsafe {
        kunwrap!(memory
            .page_mapper
            .free_mapping(arg1..(arg1 + arg2 + 0xFFF) & !0xFFF));
        Ok(0)
    }
}

unsafe fn sys_receive_event(arg1: usize, arg2: usize) -> Result<usize, SyscallError> {
    let event = kunwrap!(KernelReferenceID::from_usize(arg1));
    let thread = CPULocalStorageRW::get_current_task();

    let mode: ReceiveMode = kunwrap!(FromPrimitive::from_usize(arg2));

    let event = kunwrap!(thread.process().references.lock().references().get(&event)).clone();
    let event = kenum_cast!(event, KernelValue::Event);

    let ev = event.lock();

    let save_state = |mut event: MutexGuard<KEvent>, edge: EdgeTrigger| {
        let handle = thread.handle().clone();
        let status = handle.thread.lock();
        thread.wait_on(&mut event, edge);
        drop(event);
        Ok(block_task(status))
    };

    match mode {
        ReceiveMode::GetLevel => Ok(ev.level() as usize),
        ReceiveMode::LevelHigh => {
            if ev.level() {
                Ok(1)
            } else {
                save_state(ev, EdgeTrigger::RISING_EDGE)
            }
        }
        ReceiveMode::LevelLow => {
            if !ev.level() {
                Ok(0)
            } else {
                save_state(ev, EdgeTrigger::FALLING_EDGE)
            }
        }
        ReceiveMode::Edge => save_state(ev, EdgeTrigger::RISING_EDGE | EdgeTrigger::FALLING_EDGE),
        ReceiveMode::EdgeHigh => save_state(ev, EdgeTrigger::RISING_EDGE),
        ReceiveMode::EdgeLow => save_state(ev, EdgeTrigger::FALLING_EDGE),
    }
}

unsafe fn sys_event_queue(
    arg1: usize,
    arg2: usize,
    arg3: usize,
    arg4: usize,
    arg5: usize,
) -> Result<usize, SyscallError> {
    let thread = CPULocalStorageRW::get_current_task();

    let operation: KernelEventQueueOperation = kunwrap!(FromPrimitive::from_usize(arg1));

    let get_event = || {
        let id = kunwrap!(KernelReferenceID::from_usize(arg2));
        let event = kunwrap!(thread.process().get_value(id));
        let event = kenum_cast!(event, KernelValue::EventQueue);
        Ok(event)
    };

    match operation {
        KernelEventQueueOperation::Create => {
            let new = KEventQueue::new();
            let id = thread.process().add_value(new.into());
            Ok(id.0.get())
        }
        KernelEventQueueOperation::GetEvent => {
            let ev = get_event()?;
            let event = ev.event();
            let id = thread.process().add_value(event.into());
            Ok(id.0.get())
        }
        KernelEventQueueOperation::PopQueue => {
            let ev = get_event()?;
            match ev.try_pop_event() {
                Some(e) => Ok(e.0.get()),
                None => Ok(0),
            }
        }
        KernelEventQueueOperation::Listen => {
            let ev = get_event()?;

            let listen_id = kunwrap!(KernelReferenceID::from_usize(arg3));
            let callback = kunwrap!(NonZeroUsize::new(arg4).map(EventCallback));

            let listen_event = kunwrap!(thread.process().get_value(listen_id));
            let listen_event = kenum_cast!(listen_event, KernelValue::Event);
            let mode: KernelEventQueueListenMode = kunwrap!(FromPrimitive::from_usize(arg5));

            Ok(ev.listen(listen_event, callback, mode)?.0.get())
        }
        KernelEventQueueOperation::Unlisten => {
            let ev = get_event()?;
            let listen_id = EventQueueListenId(kunwrap!(NonZeroUsize::new(arg3)));
            kunwrap!(ev.unlisten(listen_id));
            Ok(0)
        }
    }
}

unsafe fn sys_socket_handler(arg1: usize, arg2: usize, arg3: usize) -> Result<usize, SyscallError> {
    let thread = CPULocalStorageRW::get_current_task();

    let operation: SocketOperation = kunwrap!(FromPrimitive::from_usize(arg1));

    match operation {
        SocketOperation::Listen => {
            let name = slice::from_raw_parts(arg2 as *const u8, arg3);
            let name = String::from_utf8_lossy(name).to_string();
            match PUBLIC_SOCKETS.lock().entry(name) {
                hashbrown::hash_map::Entry::Occupied(_) => Ok(0),
                hashbrown::hash_map::Entry::Vacant(place) => {
                    let handle = KSocketListener::new();
                    place.insert(handle.clone());
                    Ok(thread.process().add_value(handle.into()).0.get())
                }
            }
        }
        SocketOperation::Connect => {
            let name = slice::from_raw_parts(arg2 as *const u8, arg3);
            let name = kunwrap!(core::str::from_utf8(name));
            match PUBLIC_SOCKETS.lock().get(name) {
                Some(listener) => Ok(thread
                    .process()
                    .add_value(listener.connect().into())
                    .0
                    .get()),
                None => Ok(0),
            }
        }
        SocketOperation::Accept => {
            let id = kunwrap!(KernelReferenceID::from_usize(arg2));

            let sock = kunwrap!(thread.process().get_value(id));
            let sock = kenum_cast!(sock, KernelValue::SocketListener);

            match sock.pop() {
                Some(val) => Ok(thread.process().add_value(val.into()).0.get()),
                None => Ok(0),
            }
        }
        SocketOperation::GetSocketListenEvent => {
            let id = kunwrap!(KernelReferenceID::from_usize(arg2));

            let sock = kunwrap!(thread.process().get_value(id));
            let sock = kenum_cast!(sock, KernelValue::SocketListener);

            Ok(thread.process().add_value(sock.event().into()).0.get())
        }
        SocketOperation::Create => {
            let info = &mut *(arg2 as *mut MakeSocket);
            let sockets = create_sockets(info.ltr_capacity, info.rtl_capacity);
            let refs = &mut thread.process().references.lock();
            info.left.write(refs.add_value(sockets.0.into()));
            info.right.write(refs.add_value(sockets.1.into()));
            Ok(0)
        }
        SocketOperation::GetSocketEvent => {
            let id = kunwrap!(KernelReferenceID::from_usize(arg2));
            let ev: SocketEvents = kunwrap!(FromPrimitive::from_usize(arg3));

            let sock = kunwrap!(thread.process().get_value(id));
            let sock = kenum_cast!(sock, KernelValue::Socket);

            let event = sock.get_event(ev);

            let id = thread.process().add_value(event.into());
            Ok(id.0.get())
        }
        SocketOperation::Send => {
            let id = kunwrap!(KernelReferenceID::from_usize(arg2));
            let msgid = kunwrap!(KernelReferenceID::from_usize(arg3));

            let sock = kunwrap!(thread.process().get_value(id));
            let message = kunwrap!(thread.process().get_value(msgid));

            let sock = kenum_cast!(sock, KernelValue::Socket);
            Ok(match sock.send_message(message) {
                Some(()) => 0,
                None if sock.is_eof() => 2,
                None => 1,
            })
        }
        SocketOperation::Recv => unsafe {
            let recv = &mut *(arg2 as *mut SocketRecv);

            let sock = kunwrap!(thread.process().get_value(recv.socket));

            let sock = kenum_cast!(sock, KernelValue::Socket);

            match sock.recv_message() {
                Some(message) => {
                    recv.result_type.write(message.object_type());
                    recv.result = Some(thread.process().add_value(message));
                }
                None => recv.eof = sock.is_eof(),
            }
            Ok(0)
        },
    }
}

unsafe fn sys_reference_handler(arg1: usize, arg2: usize) -> Result<usize, SyscallError> {
    let thread = CPULocalStorageRW::get_current_task();

    let operation: ReferenceOperation = kunwrap!(FromPrimitive::from_usize(arg1));
    let id = kunwrap!(KernelReferenceID::from_usize(arg2));

    let mut refs = thread.process().references.lock();
    match operation {
        ReferenceOperation::Clone => {
            let clonable = kunwrap!(refs.references().get(&id)).clone();
            Ok(refs.add_value(clonable).0.get())
        }
        ReferenceOperation::Delete => {
            kunwrap!(refs.references().remove(&id));
            Ok(0)
        }
        ReferenceOperation::GetType => Ok(match refs.references().get(&id) {
            Some(r) => r.object_type(),
            None => kernel_userspace::object::KernelObjectType::None,
        } as usize),
    }
}

unsafe fn sys_process_handler(arg1: usize, arg2: usize) -> Result<usize, SyscallError> {
    let thread = CPULocalStorageRW::get_current_task();

    let operation: KernelProcessOperation = kunwrap!(FromPrimitive::from_usize(arg1));
    let id = kunwrap!(KernelReferenceID::from_usize(arg2));
    let proc = kunwrap!(thread.process().get_value(id));

    let proc = kenum_cast!(proc, KernelValue::Process);

    match operation {
        KernelProcessOperation::GetExitCode => Ok(*proc.exit_status.lock() as usize),
        KernelProcessOperation::GetExitEvent => Ok(thread
            .process()
            .add_value(proc.exit_signal.clone().into())
            .0
            .get()),
        KernelProcessOperation::Kill => {
            proc.kill_threads();
            Ok(0)
        }
    }
}

unsafe fn sleep_handler(arg1: usize) -> Result<usize, SyscallError> {
    let start = uptime();
    let time = start + arg1 as u64;
    let thread = CPULocalStorageRW::get_current_task();

    let handle = thread.handle().clone();
    let status = handle.thread.lock();

    SLEPT_PROCESSES
        .lock()
        .entry(time)
        .or_default()
        .push(Arc::downgrade(&handle));

    block_task(status);
    Ok((uptime() - start) as usize)
}

unsafe fn message_handler(arg1: usize, arg2: usize) -> Result<usize, SyscallError> {
    let action: SyscallMessageAction = kunwrap!(FromPrimitive::from_usize(arg1));
    let thread = CPULocalStorageRW::get_current_task();

    match action {
        SyscallMessageAction::Create => unsafe {
            let msg_create = &mut *(arg2 as *mut MessageCreate);
            let req = &msg_create.before;
            let data: Box<[u8]> = core::slice::from_raw_parts(req.0, req.1).into();
            let msg = Arc::new(KMessage { data });

            msg_create.after = thread.process().add_value(msg.into());
        },
        SyscallMessageAction::GetSize => unsafe {
            let msg_size = &mut *(arg2 as *mut MessageGetSize);

            let msg = kunwrap!(thread.process().get_value(msg_size.before));
            let msg = kenum_cast!(msg, KernelValue::Message);

            msg_size.after = msg.data.len();
        },
        SyscallMessageAction::Read => unsafe {
            let msg_read = &mut *(arg2 as *mut MessageRead);

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

    Ok(0)
}
