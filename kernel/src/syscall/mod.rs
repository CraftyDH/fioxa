use core::ptr::slice_from_raw_parts_mut;

use alloc::{boxed::Box, sync::Arc, vec::Vec};
use kernel_userspace::{
    channel::{ChannelCreate, ChannelRead, ChannelReadResult, ChannelSyscall, ChannelWrite},
    interrupt::InterruptSyscall,
    message::{MessageCreate, MessageGetSize, MessageRead, SyscallMessageAction},
    num_traits::FromPrimitive,
    object::{KernelReferenceID, ObjectSignal, ReferenceOperation, WaitPort},
    port::{PortNotification, PortSyscall},
    process::KernelProcessOperation,
    syscall::SYSCALL_NUMBER,
};
use x86_64::structures::idt::{InterruptDescriptorTable, InterruptStackFrame};

use crate::{
    channel::{channel_create, ChannelMessage, ReadError},
    cpu_localstorage::CPULocalStorageRW,
    interrupts::KInterruptHandle,
    message::KMessage,
    object::{KObject, KObjectSignal, SignalWaiter},
    paging::{
        page_allocator::{frame_alloc_exec, global_allocator},
        page_mapper::PageMapping,
        page_table::Mapper,
        AllocatedPage, GlobalPageAllocator, MemoryMappingFlags,
    },
    port::KPort,
    scheduling::{
        process::KernelValue,
        taskmanager::{self, block_task, exit_task, kill_bad_task, yield_task},
    },
    time::{uptime, SleptProcess, SLEPT_PROCESSES},
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
        {
            error!("Panicked in {}:{}:{} {}", file!(), line!(), column!(), format_args!($($arg)*));
            return Err(SyscallError::Error);
        }
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

/// Function used to ensure kernel doesn't call syscall while holding interrupts
extern "C" fn bad_interrupts_held() {
    panic!("Interrupts should not be held when entering syscall.")
}

/// Handler for internal syscalls called by the kernel (Note: not nested).
#[naked]
pub unsafe extern "C" fn syscall_kernel_handler() {
    core::arch::naked_asm!(
        "cmp qword ptr gs:0x32, 0",
        "jne {}",
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
        sym bad_interrupts_held,
        sym syscall_handler,
    );
}

/// Handler for syscalls via int 0x80
#[naked]
pub extern "x86-interrupt" fn wrapped_syscall_handler(_: InterruptStackFrame) {
    unsafe {
        core::arch::naked_asm!(
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
        );
    }
}

/// Handler for syscalls via syscall
#[naked]
pub unsafe extern "C" fn syscall_sysret_handler() {
    core::arch::naked_asm!(
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
    );
}

unsafe extern "C" fn syscall_handler(
    number: usize,
    arg1: usize,
    arg2: usize,
    arg3: usize,
    arg4: usize,
    _arg5: usize,
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
        OBJECT => sys_reference_handler(arg1, arg2, arg3),
        PROCESS => sys_process_handler(arg1, arg2),
        CHANNEL => sys_channel_handler(arg1, arg2),
        PORT => sys_port_handler(arg1, arg2, arg3),
        INTERRUPT => sys_interrupt_handler(arg1, arg2, arg3, arg4),
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

    let page = kunwrap!(frame_alloc_exec(|a| a.allocate_page_32bit()));

    let r = page.get_address() as usize;

    let mut memory = task.process().memory.lock();
    unsafe {
        memory
            .page_mapper
            .get_mapper_mut()
            .identity_map(global_allocator(), page, MemoryMappingFlags::all())
            .unwrap()
            .flush();
    }
    memory
        .owned32_pages
        .push(AllocatedPage::from_raw(page, GlobalPageAllocator));
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

unsafe fn sys_reference_handler(
    arg1: usize,
    arg2: usize,
    arg3: usize,
) -> Result<usize, SyscallError> {
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
        ReferenceOperation::Wait => {
            let val = kunwrap!(refs.references().get(&id)).clone();

            let handle = thread.handle().clone();
            let mask = ObjectSignal::from_bits_truncate(arg3 as u64);

            let waiter = |signals: &mut KObjectSignal| {
                if signals.signal_status().intersects(mask) {
                    Ok(signals.signal_status())
                } else {
                    let status = handle.thread.lock();
                    signals.wait(SignalWaiter {
                        ty: crate::object::SignalWaiterType::One(handle.clone()),
                        mask,
                    });
                    Err(status)
                }
            };

            let res = match &val {
                KernelValue::Channel(v) => v.signals(waiter),
                KernelValue::Process(v) => v.signals(waiter),
                _ => kpanic!("object not signalable"),
            };

            match res {
                Ok(val) => Ok(val.bits() as usize),
                Err(status) => {
                    drop(refs);
                    block_task(status);
                    Ok(match val {
                        KernelValue::Channel(v) => v.signals(|w| w.signal_status().bits() as usize),
                        KernelValue::Process(v) => v.signals(|w| w.signal_status().bits() as usize),
                        _ => kpanic!("object not signalable"),
                    })
                }
            }
        }
        ReferenceOperation::WaitPort => {
            let val = kunwrap!(refs.references().get(&id)).clone();

            let wait = &*(arg3 as *const WaitPort);

            let port = kunwrap!(refs.references().get(&wait.port_handle)).clone();
            let port = kenum_cast!(port, KernelValue::Port);

            let mask = ObjectSignal::from_bits_truncate(wait.mask);

            let waiter = |signals: &mut KObjectSignal| {
                if signals.signal_status().intersects(mask) {
                    port.notify(PortNotification {
                        key: wait.key,
                        ty: kernel_userspace::port::PortNotificationType::SignalOne {
                            trigger: mask,
                            signals: signals.signal_status(),
                        },
                    });
                } else {
                    signals.wait(SignalWaiter {
                        ty: crate::object::SignalWaiterType::Port {
                            port,
                            key: wait.key,
                        },
                        mask: mask,
                    });
                }
            };

            match &val {
                KernelValue::Channel(v) => v.signals(waiter),
                KernelValue::Process(v) => v.signals(waiter),
                _ => kpanic!("object not signalable"),
            };

            Ok(0)
        }
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
        .push(core::cmp::Reverse(SleptProcess {
            wakeup: time,
            thread: Arc::downgrade(&handle),
        }));

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

unsafe fn sys_channel_handler(syscall: usize, arg2: usize) -> Result<usize, SyscallError> {
    let action = kunwrap!(ChannelSyscall::from_usize(syscall));

    let thread = CPULocalStorageRW::get_current_task();

    match action {
        ChannelSyscall::Create => {
            let create = &mut *(arg2 as *mut ChannelCreate);

            let (left, right) = channel_create();

            let left = thread.process().add_value(left.into());
            let right = thread.process().add_value(right.into());

            create.left = Some(left);
            create.right = Some(right);
            Ok(1)
        }
        ChannelSyscall::Read => {
            let read = &mut *(arg2 as *mut ChannelRead);
            let handle = kunwrap!(thread.process().get_value(read.handle));

            let chan = kenum_cast!(handle, KernelValue::Channel);

            match chan.read(read.data_len, read.handles_len) {
                Ok(ok) => {
                    read.data_len = ok.data.len();
                    let data_ptr = core::slice::from_raw_parts_mut(read.data, ok.data.len());
                    data_ptr.copy_from_slice(&ok.data);

                    if let Some(h) = ok.handles {
                        read.handles_len = h.len();
                        let data_ptr: &mut [Option<KernelReferenceID>] =
                            core::slice::from_raw_parts_mut(read.handles.cast(), h.len());

                        let mut i = 0;
                        for handle in h {
                            let id = thread.process().add_value(handle);
                            data_ptr[i] = Some(id);
                            i += 1;
                        }
                    } else {
                        read.handles_len = 0;
                    }
                    Ok(ChannelReadResult::Ok as usize)
                }
                Err(ReadError::Empty) => Ok(ChannelReadResult::Empty as usize),
                Err(ReadError::Size {
                    min_bytes,
                    min_handles,
                }) => {
                    read.data_len = min_bytes;
                    read.handles_len = min_handles;
                    Ok(ChannelReadResult::Size as usize)
                }
                Err(ReadError::Closed) => Ok(ChannelReadResult::Closed as usize),
            }
        }
        ChannelSyscall::Write => {
            let write = &mut *(arg2 as *mut ChannelWrite);
            let handle = kunwrap!(thread.process().get_value(write.handle));

            let chan = kenum_cast!(handle, KernelValue::Channel);
            let data = core::slice::from_raw_parts(write.data, write.data_len);

            let handles = if !write.handles.is_null() && write.handles_len > 0 {
                let handles: &[Option<KernelReferenceID>] =
                    core::slice::from_raw_parts(write.handles.cast(), write.handles_len);
                let mut handles_res = Vec::with_capacity(write.handles_len);
                let mut refs = thread.process().references.lock();
                for h in handles {
                    match h {
                        Some(r) => handles_res.push(kunwrap!(refs.references().get(r)).clone()),
                        None => kpanic!("null ref not allowed"),
                    }
                }
                Some(handles_res.into_boxed_slice())
            } else {
                None
            };

            let msg = ChannelMessage {
                data: data.into(),
                handles,
            };
            match chan.send(msg) {
                Some(()) => Ok(1),
                None => Ok(0),
            }
        }
    }
}

unsafe fn sys_port_handler(
    syscall: usize,
    arg1: usize,
    arg2: usize,
) -> Result<usize, SyscallError> {
    let action = kunwrap!(PortSyscall::from_usize(syscall));
    let thread = CPULocalStorageRW::get_current_task();

    match action {
        PortSyscall::Create => {
            let port = KPort::new();
            let handle = thread.process().add_value(Arc::new(port).into());
            Ok(handle.0.get())
        }
        PortSyscall::Wait => {
            let handle = kunwrap!(KernelReferenceID::from_usize(arg1));
            let handle = kunwrap!(thread.process().get_value(handle));

            let port = kenum_cast!(handle, KernelValue::Port);
            let v = port.wait();
            (arg2 as *mut PortNotification).write(v);
            Ok(0)
        }
        PortSyscall::Push => {
            let handle = kunwrap!(KernelReferenceID::from_usize(arg1));
            let handle = kunwrap!(thread.process().get_value(handle));

            let port = kenum_cast!(handle, KernelValue::Port);
            let v = (arg2 as *const PortNotification).read();
            port.notify(v);
            Ok(0)
        }
    }
}

unsafe fn sys_interrupt_handler(
    syscall: usize,
    handle: usize,
    port: usize,
    key: usize,
) -> Result<usize, SyscallError> {
    let action = kunwrap!(InterruptSyscall::from_usize(syscall));

    let thread = CPULocalStorageRW::get_current_task();

    match action {
        InterruptSyscall::Create => {
            let interrupt = KInterruptHandle::new();
            let id = thread.process().add_value(Arc::new(interrupt).into());
            Ok(id.0.get())
        }
        InterruptSyscall::Trigger => {
            let id = kunwrap!(KernelReferenceID::from_usize(handle));
            let int = kunwrap!(thread.process().get_value(id));
            let int = kenum_cast!(int, KernelValue::Interrupt);
            int.trigger();
            Ok(0)
        }
        InterruptSyscall::SetPort => {
            let id = kunwrap!(KernelReferenceID::from_usize(handle));
            let int = kunwrap!(thread.process().get_value(id));
            let int = kenum_cast!(int, KernelValue::Interrupt);

            let id = kunwrap!(KernelReferenceID::from_usize(port));
            let port = kunwrap!(thread.process().get_value(id));
            let port = kenum_cast!(port, KernelValue::Port);

            int.set_port(port, key as u64);
            Ok(0)
        }
        InterruptSyscall::Acknowledge => {
            let id = kunwrap!(KernelReferenceID::from_usize(handle));
            let int = kunwrap!(thread.process().get_value(id));
            let int = kenum_cast!(int, KernelValue::Interrupt);
            int.ack();
            Ok(0)
        }
        InterruptSyscall::Wait => {
            let id = kunwrap!(KernelReferenceID::from_usize(handle));
            let int = kunwrap!(thread.process().get_value(id));
            let int = kenum_cast!(int, KernelValue::Interrupt);
            int.wait()
        }
    }
}
