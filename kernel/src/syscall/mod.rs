use alloc::{sync::Arc, vec::Vec};
use kernel_sys::{
    raw::{
        syscall::SYSCALL_NUMBER,
        types::{hid_t, pid_t, signals_t, sys_port_notification_t, tid_t, vaddr_t},
    },
    types::{
        Hid, ObjectSignal, RawValue, SysPortNotification, SysPortNotificationValue, SyscallResult,
        VMMapFlags, VMOAnonymousFlags,
    },
};
use log::Level;
use x86_64::structures::idt::{InterruptDescriptorTable, InterruptStackFrame};

use crate::{
    channel::{ChannelMessage, ReadError, channel_create},
    cpu_localstorage::CPULocalStorageRW,
    interrupts::KInterruptHandle,
    logging::print_log,
    message::KMessage,
    mutex::Spinlock,
    object::{KObject, KObjectSignal, SignalWaiter},
    port::KPort,
    scheduling::{
        process::{KernelValue, ProcessMemory, ProcessPrivilege, ProcessReferences, ThreadState},
        taskmanager::{self, enter_sched, kill_bad_task},
    },
    time::{SLEPT_PROCESSES, SleptProcess, uptime},
    user::{UserBytes, UserBytesMut, UserPtr, UserPtrMut, get_current_bounds},
    vm::VMO,
};

pub fn set_syscall_idt(idt: &mut InterruptDescriptorTable) {
    idt[SYSCALL_NUMBER]
        .set_handler_fn(wrapped_syscall_handler)
        .set_privilege_level(x86_64::PrivilegeLevel::Ring3);

    // .disable_interrupts(false);
}

#[derive(Debug)]
pub enum SyscallError {
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
            return SyscallResult::SystemError;
        }
    };
}

#[macro_export]
macro_rules! kassert {
    ($x: expr) => {
        if !$x {
            error!("KAssert failed in {}:{}:{}.", file!(), line!(), column!());
            return SyscallResult::SystemError;
        }
    };
    ($x: expr, $($arg:tt)+) => {
        if !$x {
            error!("KAssert failed in {}:{}:{} {}", file!(), line!(), column!(), format_args!($($arg)*));
            return SyscallResult::SystemError;
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
                return SyscallResult::SystemError;
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
                return SyscallResult::SystemError;
            }
        }
    };
}

/// Function used to ensure kernel doesn't call syscall while holding interrupts
extern "C" fn bad_interrupts_held() {
    panic!("Interrupts should not be held when entering syscall.")
}

/// Handler for internal syscalls called by the kernel (Note: not nested).
#[unsafe(naked)]
pub unsafe extern "C" fn syscall_kernel_handler() {
    core::arch::naked_asm!(
        // check interrupts
        "cmp qword ptr gs:0x32, 0",
        "jne {bad_interrupts_held}",

        // save regs
        "push rbp",
        "push r15",
        "pushfq",
        "cli",

        // set cpu context
        "mov r11b, 1",
        "mov gs:0x9, r11b",
        "mov r15, rsp",     // save caller rsp
        "mov rsp, gs:0x1A", // load kstack top
        "sti",

        // check bounds of syscall
        "cmp rax, {syscall_len}",
        "jb 2f",
        "mov rdi, rax",
        "call {out_of_bounds}",

        // call the syscall fn
        "2:",
        "lea r11, [rip+{syscall_fns}]",
        "call [r11+rax*8]",

        // set cpu context
        "cli",
        "mov cl, 2",
        "mov gs:0x9, cl", // set cpu context

        // restore regs
        "mov rsp, r15",   // restore caller rip
        "popfq",
        "pop r15",
        "pop rbp",
        "ret",
        syscall_len = const SYSCALL_FNS.len(),
        syscall_fns = sym SYSCALL_FNS,
        out_of_bounds = sym out_of_bounds,
        bad_interrupts_held = sym bad_interrupts_held,
    );
}

/// Handler for syscalls via int 0x80
#[unsafe(naked)]
extern "x86-interrupt" fn wrapped_syscall_handler(_: InterruptStackFrame) {
    core::arch::naked_asm!(
        // set cpu context
        "mov r11b, 1",
        "mov gs:0x9, r11b",
        "sti",

        // check bounds of syscall
        "cmp rax, {syscall_len}",
        "jb 2f",
        "mov rdi, rax",
        "call {out_of_bounds}",

        // call the syscall fn
        "2:",
        "lea r11, [rip+{syscall_fns}]",
        "call [r11+rax*8]",

        // set cpu context
        "cli",
        "mov cl, 2",
        "mov gs:0x9, cl",

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
        syscall_len = const SYSCALL_FNS.len(),
        syscall_fns = sym SYSCALL_FNS,
        out_of_bounds = sym out_of_bounds,
    );
}

/// Handler for syscalls via syscall
#[unsafe(naked)]
pub unsafe extern "C" fn syscall_sysret_handler() {
    core::arch::naked_asm!(
        // set cpu context
        "mov r12d, 1",
        "mov gs:0x9, r12d",

        // swap stack
        "mov r12, rsp",
        "mov rsp, gs:0x1A",
        "sti",

        // save registers
        "push r11", // save caller flags
        "push rcx", // save caller rip

        // move arg3 to match sysv c calling convention
        "mov rcx, r10",

        // check bounds of syscall
        "cmp rax, {syscall_len}",
        "jb 2f",
        "mov rdi, rax",
        "call {out_of_bounds}",

        // call the syscall fn
        "2:",
        "lea r11, [rip+{syscall_fns}]",
        "call [r11+rax*8]",

        // clear scratch registers (we don't want leaks)
        "xor r10d, r10d",
        "xor r9d,  r9d",
        "xor r8d,  r8d",
        "xor edi,  edi",
        "xor esi,  esi",
        "xor edx,  edx",

        // set cpu context
        "cli",
        "mov cl, 2",
        "mov gs:0x9, cl",

        // restore registers
        "pop rcx",
        "pop r11",
        "mov rsp, r12",
        "sysretq",
        syscall_len = const SYSCALL_FNS.len(),
        syscall_fns = sym SYSCALL_FNS,
        out_of_bounds = sym out_of_bounds,
    );
}

// We read into this from asm
#[allow(dead_code)]
struct SyscallFn(pub *const ());

unsafe impl Sync for SyscallFn {}

static SYSCALL_FNS: [SyscallFn; 33] = [
    // misc
    SyscallFn(handle_sys_echo as *const ()),
    SyscallFn(handle_sys_yield as *const ()),
    SyscallFn(handle_sys_sleep as *const ()),
    SyscallFn(handle_sys_exit as *const ()),
    SyscallFn(handle_sys_map as *const ()),
    SyscallFn(handle_sys_unmap as *const ()),
    SyscallFn(handle_sys_read_args as *const ()),
    SyscallFn(handle_sys_pid as *const ()),
    SyscallFn(handle_sys_log as *const ()),
    // handle
    SyscallFn(handle_sys_handle_drop as *const ()),
    SyscallFn(handle_sys_handle_clone as *const ()),
    // object
    SyscallFn(handle_sys_object_type as *const ()),
    SyscallFn(handle_sys_object_wait as *const ()),
    SyscallFn(handle_sys_object_wait_port as *const ()),
    // channel
    SyscallFn(handle_sys_channel_create as *const ()),
    SyscallFn(handle_sys_channel_read as *const ()),
    SyscallFn(handle_sys_channel_write as *const ()),
    // interrupt
    SyscallFn(handle_sys_interrupt_create as *const ()),
    SyscallFn(handle_sys_interrupt_wait as *const ()),
    SyscallFn(handle_sys_interrupt_trigger as *const ()),
    SyscallFn(handle_sys_interrupt_acknowledge as *const ()),
    SyscallFn(handle_sys_interrupt_set_port as *const ()),
    // port
    SyscallFn(handle_sys_port_create as *const ()),
    SyscallFn(handle_sys_port_wait as *const ()),
    SyscallFn(handle_sys_port_push as *const ()),
    // process
    SyscallFn(handle_sys_process_spawn_thread as *const ()),
    SyscallFn(handle_sys_process_exit_code as *const ()),
    // message
    SyscallFn(handle_sys_message_create as *const ()),
    SyscallFn(handle_sys_message_size as *const ()),
    SyscallFn(handle_sys_message_read as *const ()),
    // vmo
    SyscallFn(handle_sys_vmo_mmap_create as *const ()),
    SyscallFn(handle_sys_vmo_anonymous_create as *const ()),
    SyscallFn(handle_sys_vmo_anonymous_pinned_addresses as *const ()),
];

extern "C" fn out_of_bounds(number: usize) -> ! {
    info!("Out of bounds syscall {number}");
    kill_bad_task();
}

extern "C" fn handle_sys_echo(val: usize) -> usize {
    info!("ECHO {val}");
    val
}

unsafe extern "C" fn handle_sys_yield() {
    let thread = unsafe { CPULocalStorageRW::get_current_task() };
    let mut sched = thread.sched().lock();
    enter_sched(&mut sched);
}

unsafe extern "C" fn handle_sys_sleep(ms: u64) -> u64 {
    let start = uptime();
    let time = start + ms;
    let thread = unsafe { CPULocalStorageRW::get_current_task() };

    let mut sched = thread.sched().lock();
    sched.state = ThreadState::Sleeping;

    SLEPT_PROCESSES
        .lock()
        .push(core::cmp::Reverse(SleptProcess {
            wakeup: time,
            thread: thread.thread(),
        }));

    enter_sched(&mut sched);
    uptime() - start
}

unsafe extern "C" fn handle_sys_exit() -> ! {
    let thread = unsafe { CPULocalStorageRW::get_current_task() };
    let mut sched = thread.sched().lock();
    sched.state = ThreadState::Killed;
    enter_sched(&mut sched);
    unreachable!("exit thread shouldn't return")
}

unsafe extern "C" fn handle_sys_map(
    vmo: hid_t,
    flags: u32,
    hint: vaddr_t,
    length: usize,
    result: *mut vaddr_t,
) -> SyscallResult {
    let task = unsafe { CPULocalStorageRW::get_current_task() };
    let bounds = get_current_bounds(&task.process());
    kassert!(hint as usize + length <= bounds.top());
    let mut result = kunwrap!(unsafe { UserPtrMut::new(result, bounds) });

    let memory: &mut ProcessMemory = &mut task.process().memory.lock();
    let refs: &mut ProcessReferences = &mut task.process().references.lock();

    let flags = VMMapFlags::from_bits_truncate(flags);

    let vmo_handle = match Hid::from_raw(vmo) {
        Ok(vmo) => {
            let val = kunwrap!(refs.references().get(&vmo));
            kenum_cast!(val, KernelValue::VMO).clone()
        }
        Err(_) => {
            // allocate anonymous object for the mapping
            Arc::new(Spinlock::new(VMO::new_anonymous(
                length,
                VMOAnonymousFlags::empty(),
            )))
        }
    };

    let hint = if hint.is_null() {
        None
    } else {
        Some(hint as usize)
    };

    match memory.region.map_vmo(vmo_handle, flags, hint) {
        Ok(res) => result.write(res as *mut ()),
        Err(e) => {
            error!("Err {e:?}");
            return SyscallResult::BadInputPointer;
        }
    }

    SyscallResult::Ok
}

unsafe extern "C" fn handle_sys_unmap(address: vaddr_t, length: usize) -> SyscallResult {
    let task = unsafe { CPULocalStorageRW::get_current_task() };
    let bounds = get_current_bounds(&task.process());
    kassert!(address as usize + length <= bounds.top());

    let memory: &mut ProcessMemory = &mut task.process().memory.lock();

    match unsafe { memory.region.unmap(address as usize, length) } {
        Ok(()) => SyscallResult::Ok,
        Err(err) => {
            info!("Error unmapping: {address:?}-{length} {err:?}");
            SyscallResult::BadInputPointer
        }
    }
}

unsafe extern "C" fn handle_sys_read_args(buffer: *mut u8, len: usize) -> usize {
    let task = unsafe { CPULocalStorageRW::get_current_task() };
    let bounds = get_current_bounds(&task.process());

    let result = unsafe { UserBytesMut::new(buffer, len, bounds) };
    let Some(mut result) = result else {
        kill_bad_task();
    };

    let proc = task.process();
    let bytes = &proc.args;

    if buffer.is_null() || len != bytes.len() {
        return bytes.len();
    }

    result.write(&bytes);

    usize::MAX
}

unsafe extern "C" fn handle_sys_pid() -> pid_t {
    unsafe {
        CPULocalStorageRW::get_current_task()
            .process()
            .pid
            .into_raw()
    }
}

unsafe extern "C" fn handle_sys_log(
    level: u32,
    target: *const u8,
    target_len: usize,
    message: *const u8,
    message_len: usize,
) {
    unsafe {
        let task = CPULocalStorageRW::get_current_task();
        let bounds = get_current_bounds(&task.process());

        let target = UserBytes::new(target, target_len, bounds);
        let message = UserBytes::new(message, message_len, bounds);

        let Some((target, message)) = target.zip(message) else {
            warn!("Out of bounds log");
            kill_bad_task();
        };

        let target = target.read_to_box();
        let message = message.read_to_box();

        let target = core::str::from_utf8(&target);
        let message = core::str::from_utf8(&message);

        let Some((target, message)) = target.ok().zip(message.ok()) else {
            warn!("non utf-8 log");
            kill_bad_task();
        };

        let level = match level {
            1 => Level::Error,
            2 => Level::Warn,
            3 => Level::Info,
            4 => Level::Debug,
            5 => Level::Trace,
            _ => {
                warn!("Invalid level {level}");
                kill_bad_task();
            }
        };

        print_log(level, target, &format_args!("{message}"));
    }
}

// handle

unsafe extern "C" fn handle_sys_handle_drop(handle: hid_t) -> SyscallResult {
    let thread = unsafe { CPULocalStorageRW::get_current_task() };
    let refs: &mut ProcessReferences = &mut thread.process().references.lock();

    let handle = kunwrap!(Hid::from_raw(handle));

    match refs.references().remove(&handle) {
        Some(_) => SyscallResult::Ok,
        None => SyscallResult::UnknownHandle,
    }
}

unsafe extern "C" fn handle_sys_handle_clone(handle: hid_t, cloned: *mut hid_t) -> SyscallResult {
    let thread = unsafe { CPULocalStorageRW::get_current_task() };
    let bounds = get_current_bounds(&thread.process());
    let mut cloned = unsafe { kunwrap!(UserPtrMut::new(cloned, bounds)) };

    let refs: &mut ProcessReferences = &mut thread.process().references.lock();

    let handle = kunwrap!(Hid::from_raw(handle));

    match refs.references().get(&handle).cloned() {
        Some(h) => {
            let new = refs.add_value(h);
            cloned.write(new.0.get());
            SyscallResult::Ok
        }
        None => SyscallResult::UnknownHandle,
    }
}

// object

unsafe extern "C" fn handle_sys_object_type(handle: hid_t, ty: *mut usize) -> SyscallResult {
    let thread = unsafe { CPULocalStorageRW::get_current_task() };
    let bounds = get_current_bounds(&thread.process());
    let mut ty = unsafe { kunwrap!(UserPtrMut::new(ty, bounds)) };

    let refs: &mut ProcessReferences = &mut thread.process().references.lock();

    let handle = kunwrap!(Hid::from_raw(handle));

    match refs.references().get(&handle) {
        Some(h) => {
            ty.write(h.object_type() as usize);
            SyscallResult::Ok
        }
        None => SyscallResult::UnknownHandle,
    }
}
unsafe extern "C" fn handle_sys_object_wait(
    handle: hid_t,
    on: signals_t,
    result: *mut signals_t,
) -> SyscallResult {
    let thread = unsafe { CPULocalStorageRW::get_current_task() };
    let bounds = get_current_bounds(&thread.process());
    let mut result = unsafe { kunwrap!(UserPtrMut::new(result, bounds)) };

    let mut refs = thread.process().references.lock();

    let handle = kunwrap!(Hid::from_raw(handle));

    let Some(val) = refs.references().get(&handle).cloned() else {
        return SyscallResult::UnknownHandle;
    };

    let mask = ObjectSignal::from_bits_truncate(on);

    let waiter = |signals: &mut KObjectSignal| {
        if signals.signal_status().intersects(mask) {
            Ok(signals.signal_status())
        } else {
            let mut sched = thread.sched().lock();
            sched.state = ThreadState::Sleeping;
            signals.wait(SignalWaiter {
                ty: crate::object::SignalWaiterType::One(thread.thread()),
                mask,
            });
            Err(sched)
        }
    };

    let res = match &val {
        KernelValue::Channel(v) => v.signals(waiter),
        KernelValue::Process(v) => v.signals(waiter),
        _ => kpanic!("object not signalable"),
    };

    match res {
        Ok(val) => result.write(val.bits()),
        Err(mut status) => {
            drop(refs);
            enter_sched(&mut status);
            result.write(match val {
                KernelValue::Channel(v) => v.signals(|w| w.signal_status().bits()),
                KernelValue::Process(v) => v.signals(|w| w.signal_status().bits()),
                _ => kpanic!("object not signalable"),
            })
        }
    }
    SyscallResult::Ok
}

unsafe extern "C" fn handle_sys_object_wait_port(
    handle: hid_t,
    port: hid_t,
    on: signals_t,
    key: u64,
) -> SyscallResult {
    let thread = unsafe { CPULocalStorageRW::get_current_task() };
    let refs: &mut ProcessReferences = &mut thread.process().references.lock();

    let handle = kunwrap!(Hid::from_raw(handle));
    let port = kunwrap!(Hid::from_raw(port));

    let refs = refs.references();
    let Some(handle) = refs.get(&handle) else {
        return SyscallResult::UnknownHandle;
    };

    let Some(port) = refs.get(&port) else {
        return SyscallResult::UnknownHandle;
    };

    let mask = ObjectSignal::from_bits_truncate(on);

    let port = kenum_cast!(port, KernelValue::Port);

    let waiter = |signals: &mut KObjectSignal| {
        if signals.signal_status().intersects(mask) {
            port.notify(SysPortNotification {
                key: key,
                value: SysPortNotificationValue::SignalOne {
                    trigger: mask,
                    signals: signals.signal_status(),
                },
            });
        } else {
            signals.wait(SignalWaiter {
                ty: crate::object::SignalWaiterType::Port {
                    port: port.clone(),
                    key: key,
                },
                mask: mask,
            });
        }
    };

    match &handle {
        KernelValue::Channel(v) => v.signals(waiter),
        KernelValue::Process(v) => v.signals(waiter),
        _ => kpanic!("object not signalable"),
    };

    SyscallResult::Ok
}

// channel

unsafe extern "C" fn handle_sys_channel_create(left: *mut hid_t, right: *mut hid_t) {
    let thread = unsafe { CPULocalStorageRW::get_current_task() };

    let bounds = get_current_bounds(&thread.process());
    let left = unsafe { UserPtrMut::new(left, bounds) };
    let right = unsafe { UserPtrMut::new(right, bounds) };

    let Some((mut left, mut right)) = left.zip(right) else {
        warn!("Out of bounds pointers");
        kill_bad_task();
    };

    let (l, r) = channel_create();

    let l = thread.process().add_value(l.into());
    let r = thread.process().add_value(r.into());

    left.write(l.into_raw());
    right.write(r.into_raw());
}

unsafe extern "C" fn handle_sys_channel_read(
    handle: hid_t,
    data: *mut u8,
    data_len: *mut usize,
    handles: *mut hid_t,
    handles_len: *mut usize,
) -> SyscallResult {
    let thread = unsafe { CPULocalStorageRW::get_current_task() };
    let bounds = get_current_bounds(&thread.process());

    let mut data_len = unsafe { kunwrap!(UserPtrMut::new(data_len, bounds)) };
    let mut handles_len = unsafe { kunwrap!(UserPtrMut::new(handles_len, bounds)) };

    let handle = kunwrap!(Hid::from_raw(handle));
    let handle = kunwrap!(thread.process().get_value(handle));
    let chan = kenum_cast!(handle, KernelValue::Channel);

    match chan.read(data_len.read(), handles_len.read()) {
        Ok(ok) => {
            data_len.write(ok.data.len());
            let mut data_buf = unsafe { kunwrap!(UserBytesMut::new(data, ok.data.len(), bounds)) };

            data_buf.write(&ok.data);

            if let Some(h) = ok.handles {
                handles_len.write(h.len());

                let mut handle_ptr = handles;
                for handle in h {
                    let id = thread.process().add_value(handle);
                    let mut h = unsafe { kunwrap!(UserPtrMut::new(handle_ptr, bounds)) };
                    h.write(id.into_raw());
                    handle_ptr = handle_ptr.wrapping_add(1);
                }
            } else {
                handles_len.write(0);
            }
            SyscallResult::Ok
        }
        Err(ReadError::Empty) => SyscallResult::ChannelEmpty,
        Err(ReadError::Size {
            min_bytes,
            min_handles,
        }) => {
            data_len.write(min_bytes);
            handles_len.write(min_handles);
            SyscallResult::ChannelBufferTooSmall
        }
        Err(ReadError::Closed) => SyscallResult::ChannelClosed,
    }
}

unsafe extern "C" fn handle_sys_channel_write(
    handle: hid_t,
    data: *const u8,
    data_len: usize,
    handles: *const hid_t,
    handles_len: usize,
) -> SyscallResult {
    let thread = unsafe { CPULocalStorageRW::get_current_task() };
    let bounds = get_current_bounds(&thread.process());
    let data = unsafe { kunwrap!(UserBytes::new(data, data_len, bounds)) };

    let handle = kunwrap!(Hid::from_raw(handle));
    let handle = kunwrap!(thread.process().get_value(handle));
    let chan = kenum_cast!(handle, KernelValue::Channel);

    let handles = if !handles.is_null() && handles_len > 0 {
        let mut handles_res = Vec::with_capacity(handles_len);
        let mut refs = thread.process().references.lock();
        for i in 0..handles_len {
            let h = unsafe { kunwrap!(UserPtr::new(handles.wrapping_add(i), bounds)) };
            let r = kunwrap!(Hid::from_raw(h.read()));
            handles_res.push(kunwrap!(refs.references().get(&r)).clone());
        }
        Some(handles_res.into_boxed_slice())
    } else {
        None
    };

    let msg = ChannelMessage {
        data: data.read_to_box(),
        handles,
    };
    chan.send(msg)
}

unsafe extern "C" fn handle_sys_interrupt_create() -> hid_t {
    let thread = unsafe { CPULocalStorageRW::get_current_task() };

    let interrupt = KInterruptHandle::new();
    let id = thread.process().add_value(Arc::new(interrupt).into());
    id.0.get()
}

unsafe extern "C" fn handle_sys_interrupt_wait(handle: hid_t) -> SyscallResult {
    let thread = unsafe { CPULocalStorageRW::get_current_task() };

    let handle = kunwrap!(Hid::from_raw(handle));
    let int = kunwrap!(thread.process().get_value(handle));
    let int = kenum_cast!(int, KernelValue::Interrupt);
    int.wait()
}

unsafe extern "C" fn handle_sys_interrupt_trigger(handle: hid_t) -> SyscallResult {
    let thread = unsafe { CPULocalStorageRW::get_current_task() };

    let handle = kunwrap!(Hid::from_raw(handle));
    let int = kunwrap!(thread.process().get_value(handle));
    let int = kenum_cast!(int, KernelValue::Interrupt);
    int.trigger();
    SyscallResult::Ok
}

unsafe extern "C" fn handle_sys_interrupt_acknowledge(handle: hid_t) -> SyscallResult {
    let thread = unsafe { CPULocalStorageRW::get_current_task() };

    let handle = kunwrap!(Hid::from_raw(handle));
    let int = kunwrap!(thread.process().get_value(handle));
    let int = kenum_cast!(int, KernelValue::Interrupt);
    int.ack();
    SyscallResult::Ok
}

unsafe extern "C" fn handle_sys_interrupt_set_port(
    handle: hid_t,
    port: hid_t,
    key: u64,
) -> SyscallResult {
    let thread = unsafe { CPULocalStorageRW::get_current_task() };

    let int = kunwrap!(Hid::from_raw(handle));
    let int = kunwrap!(thread.process().get_value(int));
    let int = kenum_cast!(int, KernelValue::Interrupt);

    let port = kunwrap!(Hid::from_raw(port));
    let port = kunwrap!(thread.process().get_value(port));
    let port = kenum_cast!(port, KernelValue::Port);
    int.set_port(port, key);
    SyscallResult::Ok
}

// port

unsafe extern "C" fn handle_sys_port_create() -> hid_t {
    let thread = unsafe { CPULocalStorageRW::get_current_task() };

    let interrupt = KPort::new();
    let id = thread.process().add_value(Arc::new(interrupt).into());
    id.0.get()
}

unsafe extern "C" fn handle_sys_port_wait(
    handle: hid_t,
    result: *mut sys_port_notification_t,
) -> SyscallResult {
    let thread = unsafe { CPULocalStorageRW::get_current_task() };
    let bounds = get_current_bounds(&thread.process());
    let mut result = unsafe { kunwrap!(UserPtrMut::new(result, bounds)) };

    let handle = kunwrap!(Hid::from_raw(handle));
    let handle = kunwrap!(thread.process().get_value(handle));
    let port = kenum_cast!(handle, KernelValue::Port);

    result.write(port.wait().into_raw());

    SyscallResult::Ok
}

unsafe extern "C" fn handle_sys_port_push(
    handle: hid_t,
    value: *const sys_port_notification_t,
) -> SyscallResult {
    let thread = unsafe { CPULocalStorageRW::get_current_task() };
    let bounds = get_current_bounds(&thread.process());
    let value = unsafe { kunwrap!(UserPtr::new(value, bounds)) };

    let handle = kunwrap!(Hid::from_raw(handle));
    let handle = kunwrap!(thread.process().get_value(handle));
    let port = kenum_cast!(handle, KernelValue::Port);

    let value = kunwrap!(SysPortNotification::from_raw(value.read()));

    port.notify(value);

    SyscallResult::Ok
}

// process

unsafe extern "C" fn handle_sys_process_spawn_thread(func: *const (), arg: *const ()) -> tid_t {
    unsafe { taskmanager::spawn_thread(func as usize, arg as usize).into_raw() }
}

unsafe extern "C" fn handle_sys_process_exit_code(
    handle: hid_t,
    exit: *mut usize,
) -> SyscallResult {
    let thread = unsafe { CPULocalStorageRW::get_current_task() };
    let bounds = get_current_bounds(&thread.process());
    let mut exit = unsafe { kunwrap!(UserPtrMut::new(exit, bounds)) };

    let handle = kunwrap!(Hid::from_raw(handle));
    let handle = kunwrap!(thread.process().get_value(handle));
    let proc = kenum_cast!(handle, KernelValue::Process);
    let status = *proc.exit_status.lock();
    match status {
        Some(val) => {
            exit.write(val);
            SyscallResult::Ok
        }
        None => SyscallResult::ProcessStillRunning,
    }
}

// message

unsafe extern "C" fn handle_sys_message_create(data: *const u8, len: usize) -> hid_t {
    let thread = unsafe { CPULocalStorageRW::get_current_task() };
    let bounds = get_current_bounds(&thread.process());
    let data = unsafe { UserBytes::new(data, len, bounds) };
    let Some(data) = data else {
        warn!("Bad message ptr");
        kill_bad_task();
    };

    let msg = Arc::new(KMessage {
        data: data.read_to_box(),
    });

    thread.process().add_value(msg.into()).into_raw()
}

unsafe extern "C" fn handle_sys_message_size(handle: hid_t, size: *mut usize) -> SyscallResult {
    let thread = unsafe { CPULocalStorageRW::get_current_task() };
    let bounds = get_current_bounds(&thread.process());
    let mut size = unsafe { kunwrap!(UserPtrMut::new(size, bounds)) };

    let handle = kunwrap!(Hid::from_raw(handle));
    let handle = kunwrap!(thread.process().get_value(handle));
    let message = kenum_cast!(handle, KernelValue::Message);
    size.write(message.data.len());
    SyscallResult::Ok
}

unsafe extern "C" fn handle_sys_message_read(
    handle: hid_t,
    buffer: *mut u8,
    buf_len: usize,
) -> SyscallResult {
    let thread = unsafe { CPULocalStorageRW::get_current_task() };
    let bounds = get_current_bounds(&thread.process());
    let mut buffer = unsafe { kunwrap!(UserBytesMut::new(buffer, buf_len, bounds)) };

    let handle = kunwrap!(Hid::from_raw(handle));
    let handle = kunwrap!(thread.process().get_value(handle));
    let message = kenum_cast!(handle, KernelValue::Message);

    kassert!(
        message.data.len() == buf_len,
        "Data and loc len should be same instead was: {} {}",
        message.data.len(),
        buf_len
    );

    buffer.write(&message.data);

    SyscallResult::Ok
}

// vmo

unsafe extern "C" fn handle_sys_vmo_mmap_create(base: *mut (), length: usize) -> hid_t {
    unsafe {
        let thread = CPULocalStorageRW::get_current_task();

        if thread.process().privilege != ProcessPrivilege::KERNEL {
            warn!("MMAP is privileged");
            kill_bad_task()
        }

        let vmo = Arc::new(Spinlock::new(VMO::new_mmap(base as usize, length)));
        thread
            .process()
            .references
            .lock()
            .add_value(vmo.into())
            .into_raw()
    }
}

unsafe extern "C" fn handle_sys_vmo_anonymous_create(length: usize, flags: u32) -> hid_t {
    let thread = unsafe { CPULocalStorageRW::get_current_task() };

    let flags = VMOAnonymousFlags::from_bits_truncate(flags);

    if flags.intersects(VMOAnonymousFlags::_PRIVILEGED)
        && thread.process().privilege == ProcessPrivilege::USER
    {
        warn!("Only kernel can use privileged flags");
        kill_bad_task();
    }

    let vmo = Arc::new(Spinlock::new(VMO::new_anonymous(length, flags)));
    thread
        .process()
        .references
        .lock()
        .add_value(vmo.into())
        .into_raw()
}

unsafe extern "C" fn handle_sys_vmo_anonymous_pinned_addresses(
    handle: hid_t,
    offset: usize,
    length: usize,
    result: *mut usize,
) -> SyscallResult {
    let thread = unsafe { CPULocalStorageRW::get_current_task() };

    if thread.process().privilege == ProcessPrivilege::USER {
        warn!("Only kernel can use");
        kill_bad_task();
    }

    let handle = kunwrap!(Hid::from_raw(handle));

    let mut refs = thread.process().references.lock();
    let val = kunwrap!(refs.references().get(&handle));
    let vmo = kenum_cast!(val, KernelValue::VMO);
    match &*vmo.lock() {
        VMO::MemoryMapped { .. } => kpanic!("not anonymous"),
        VMO::Anonymous { flags, pages } => {
            kassert!(flags.contains(VMOAnonymousFlags::PINNED));
            let bounds = get_current_bounds(&thread.process());

            for (i, p) in pages.iter().skip(offset).enumerate().take(length) {
                let mut ptr = unsafe { kunwrap!(UserPtrMut::new(result.add(i), bounds)) };
                ptr.write(p.map(|v| v.get_address() as usize).unwrap_or(0));
            }
        }
    }

    SyscallResult::Ok
}
