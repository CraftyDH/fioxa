use core::ptr::slice_from_raw_parts_mut;

use alloc::{boxed::Box, sync::Arc, vec::Vec};
use kernel_sys::{
    raw::{
        syscall::SYSCALL_NUMBER,
        types::{hid_t, pid_t, signals_t, sys_port_notification_t, tid_t, vaddr_t},
    },
    types::{
        Hid, MapMemoryFlags, ObjectSignal, RawValue, SysPortNotification, SysPortNotificationValue,
        SyscallResult,
    },
};
use x86_64::structures::idt::{InterruptDescriptorTable, InterruptStackFrame};

use crate::{
    channel::{channel_create, ChannelMessage, ReadError},
    cpu_localstorage::CPULocalStorageRW,
    interrupts::KInterruptHandle,
    message::KMessage,
    object::{KObject, KObjectSignal, SignalWaiter},
    paging::{
        page_allocator::frame_alloc_exec, page_mapper::PageMapping, AllocatedPage,
        GlobalPageAllocator, MemoryMappingFlags,
    },
    port::KPort,
    scheduling::{
        process::{KernelValue, ProcessMemory, ProcessReferences, ThreadState},
        taskmanager::{self, enter_sched, kill_bad_task},
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
macro_rules! kpanic2 {
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
macro_rules! kassert2 {
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
                return Err(SyscallError::Error);
            }
        }
    };
}

#[macro_export]
macro_rules! kunwrap2 {
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
                return Err(SyscallError::Error);
            }
        }
    };
}

#[macro_export]
macro_rules! kenum_cast2 {
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
#[naked]
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
#[naked]
pub extern "x86-interrupt" fn wrapped_syscall_handler(_: InterruptStackFrame) {
    unsafe {
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
}

/// Handler for syscalls via syscall
#[naked]
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

static SYSCALL_FNS: [SyscallFn; 29] = [
    // misc
    SyscallFn(handle_sys_echo as *const ()),
    SyscallFn(handle_sys_yield as *const ()),
    SyscallFn(handle_sys_sleep as *const ()),
    SyscallFn(handle_sys_exit as *const ()),
    SyscallFn(handle_sys_map as *const ()),
    SyscallFn(handle_sys_unmap as *const ()),
    SyscallFn(handle_sys_read_args as *const ()),
    SyscallFn(handle_sys_pid as *const ()),
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
    let thread = CPULocalStorageRW::get_current_task();
    let mut sched = thread.sched().lock();
    enter_sched(&mut sched);
}

unsafe extern "C" fn handle_sys_sleep(ms: u64) -> u64 {
    let start = uptime();
    let time = start + ms;
    let thread = CPULocalStorageRW::get_current_task();

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
    let thread = CPULocalStorageRW::get_current_task();
    let mut sched = thread.sched().lock();
    sched.state = ThreadState::Killed;
    enter_sched(&mut sched);
    unreachable!("exit thread shouldn't return")
}

unsafe extern "C" fn handle_sys_map(
    mut hint: vaddr_t,
    length: usize,
    flags: u32,
    result: *mut vaddr_t,
) -> SyscallResult {
    kassert2!(hint as usize <= crate::paging::MemoryLoc::EndUserMem as usize);

    let task = CPULocalStorageRW::get_current_task();

    let memory: &mut ProcessMemory = &mut task.process().memory.lock();

    let flags = MapMemoryFlags::from_bits_truncate(flags);

    let mapping = if flags.contains(MapMemoryFlags::ALLOC_32BITS) {
        kassert2!(length <= 0x1000);
        let page = frame_alloc_exec(|a| a.allocate_page_32bit()).unwrap();

        // when alloc 32 bit we identity map so userspace knows the addr
        hint = page.get_address() as _;

        PageMapping::new_lazy_prealloc(Box::new([Some(AllocatedPage::from_raw(
            page,
            GlobalPageAllocator,
        ))]))
    } else if flags.contains(MapMemoryFlags::PREALLOC) {
        PageMapping::new_lazy_filled((length + 0xFFF) & !0xFFF)
    } else {
        PageMapping::new_lazy((length + 0xFFF) & !0xFFF)
    };

    let mut map_flags = MemoryMappingFlags::USERSPACE;

    if flags.contains(MapMemoryFlags::WRITEABLE) {
        map_flags |= MemoryMappingFlags::WRITEABLE;
    }

    if hint.is_null() {
        let addr = memory.page_mapper.insert_mapping(mapping, map_flags);
        *result = addr as *mut ();
    } else {
        match memory
            .page_mapper
            .insert_mapping_at(hint as usize, mapping, map_flags)
        {
            Some(()) => *result = hint,
            None => return SyscallResult::BadInputPointer,
        }
    }

    SyscallResult::Ok
}

unsafe extern "C" fn handle_sys_unmap(address: vaddr_t, length: usize) -> SyscallResult {
    kassert2!(address as usize <= crate::paging::MemoryLoc::EndUserMem as usize);

    let task = CPULocalStorageRW::get_current_task();

    let memory: &mut ProcessMemory = &mut task.process().memory.lock();

    match memory
        .page_mapper
        .free_mapping(address as usize..(address as usize + length + 0xFFF) & !0xFFF)
    {
        Ok(()) => SyscallResult::Ok,
        Err(err) => {
            info!("Error unmapping: {address:?}-{length} {err:?}");
            SyscallResult::BadInputPointer
        }
    }
}

unsafe extern "C" fn handle_sys_read_args(buffer: *mut u8, len: usize) -> usize {
    let task = CPULocalStorageRW::get_current_task();

    let proc = task.process();
    let bytes = &proc.args;

    if buffer.is_null() || len != bytes.len() {
        return bytes.len();
    }
    let buf = unsafe { &mut *slice_from_raw_parts_mut(buffer, len) };
    buf.copy_from_slice(bytes);
    usize::MAX
}

unsafe extern "C" fn handle_sys_pid() -> pid_t {
    CPULocalStorageRW::get_current_task()
        .process()
        .pid
        .into_raw()
}

// handle

unsafe extern "C" fn handle_sys_handle_drop(handle: hid_t) -> SyscallResult {
    let thread = CPULocalStorageRW::get_current_task();
    let refs: &mut ProcessReferences = &mut thread.process().references.lock();

    let handle = kunwrap2!(Hid::from_raw(handle));

    match refs.references().remove(&handle) {
        Some(_) => SyscallResult::Ok,
        None => SyscallResult::UnknownHandle,
    }
}

unsafe extern "C" fn handle_sys_handle_clone(handle: hid_t, cloned: *mut hid_t) -> SyscallResult {
    let thread = CPULocalStorageRW::get_current_task();
    let refs: &mut ProcessReferences = &mut thread.process().references.lock();

    let handle = kunwrap2!(Hid::from_raw(handle));

    match refs.references().get(&handle).cloned() {
        Some(h) => {
            let new = refs.add_value(h);
            *cloned = new.0.get();
            SyscallResult::Ok
        }
        None => SyscallResult::UnknownHandle,
    }
}

// object

unsafe extern "C" fn handle_sys_object_type(handle: hid_t, ty: *mut usize) -> SyscallResult {
    let thread = CPULocalStorageRW::get_current_task();
    let refs: &mut ProcessReferences = &mut thread.process().references.lock();

    let handle = kunwrap2!(Hid::from_raw(handle));

    match refs.references().get(&handle) {
        Some(h) => {
            *ty = h.object_type() as usize;
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
    let thread = CPULocalStorageRW::get_current_task();
    let mut refs = thread.process().references.lock();

    let handle = kunwrap2!(Hid::from_raw(handle));

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
        _ => kpanic2!("object not signalable"),
    };

    match res {
        Ok(val) => *result = val.bits(),
        Err(mut status) => {
            drop(refs);
            enter_sched(&mut status);
            *result = match val {
                KernelValue::Channel(v) => v.signals(|w| w.signal_status().bits()),
                KernelValue::Process(v) => v.signals(|w| w.signal_status().bits()),
                _ => kpanic2!("object not signalable"),
            }
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
    let thread = CPULocalStorageRW::get_current_task();
    let refs: &mut ProcessReferences = &mut thread.process().references.lock();

    let handle = kunwrap2!(Hid::from_raw(handle));
    let port = kunwrap2!(Hid::from_raw(port));

    let refs = refs.references();
    let Some(handle) = refs.get(&handle) else {
        return SyscallResult::UnknownHandle;
    };

    let Some(port) = refs.get(&port) else {
        return SyscallResult::UnknownHandle;
    };

    let mask = ObjectSignal::from_bits_truncate(on);

    let port = kenum_cast2!(port, KernelValue::Port);

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
        _ => kpanic2!("object not signalable"),
    };

    SyscallResult::Ok
}

// channel

unsafe extern "C" fn handle_sys_channel_create(left: *mut hid_t, right: *mut hid_t) {
    let thread = CPULocalStorageRW::get_current_task();
    let (l, r) = channel_create();

    let l = thread.process().add_value(l.into());
    let r = thread.process().add_value(r.into());

    *left = l.into_raw();
    *right = r.into_raw();
}

unsafe extern "C" fn handle_sys_channel_read(
    handle: hid_t,
    data: *mut u8,
    data_len: *mut usize,
    handles: *mut hid_t,
    handles_len: *mut usize,
) -> SyscallResult {
    let thread = CPULocalStorageRW::get_current_task();
    let handle = kunwrap2!(Hid::from_raw(handle));
    let handle = kunwrap2!(thread.process().get_value(handle));
    let chan = kenum_cast2!(handle, KernelValue::Channel);

    match chan.read(*data_len, *handles_len) {
        Ok(ok) => {
            *data_len = ok.data.len();
            let data_ptr = core::slice::from_raw_parts_mut(data, ok.data.len());
            data_ptr.copy_from_slice(&ok.data);

            if let Some(h) = ok.handles {
                *handles_len = h.len();
                let data_ptr: &mut [hid_t] = core::slice::from_raw_parts_mut(handles, h.len());

                let mut i = 0;
                for handle in h {
                    let id = thread.process().add_value(handle);
                    data_ptr[i] = id.into_raw();
                    i += 1;
                }
            } else {
                *handles_len = 0;
            }
            SyscallResult::Ok
        }
        Err(ReadError::Empty) => SyscallResult::ChannelEmpty,
        Err(ReadError::Size {
            min_bytes,
            min_handles,
        }) => {
            *data_len = min_bytes;
            *handles_len = min_handles;
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
    let thread = CPULocalStorageRW::get_current_task();
    let handle = kunwrap2!(Hid::from_raw(handle));
    let handle = kunwrap2!(thread.process().get_value(handle));
    let chan = kenum_cast2!(handle, KernelValue::Channel);

    let data = core::slice::from_raw_parts(data, data_len);

    let handles = if !handles.is_null() && handles_len > 0 {
        let handles: &[hid_t] = core::slice::from_raw_parts(handles, handles_len);
        let mut handles_res = Vec::with_capacity(handles_len);
        let mut refs = thread.process().references.lock();
        for h in handles {
            let r = kunwrap2!(Hid::from_raw(*h));
            handles_res.push(kunwrap2!(refs.references().get(&r)).clone());
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
        Some(()) => SyscallResult::Ok,
        None => SyscallResult::ChannelFull,
    }
}

unsafe extern "C" fn handle_sys_interrupt_create() -> hid_t {
    let thread = CPULocalStorageRW::get_current_task();

    let interrupt = KInterruptHandle::new();
    let id = thread.process().add_value(Arc::new(interrupt).into());
    id.0.get()
}

unsafe extern "C" fn handle_sys_interrupt_wait(handle: hid_t) -> SyscallResult {
    let thread = CPULocalStorageRW::get_current_task();

    let handle = kunwrap2!(Hid::from_raw(handle));
    let int = kunwrap2!(thread.process().get_value(handle));
    let int = kenum_cast2!(int, KernelValue::Interrupt);
    int.wait()
}

unsafe extern "C" fn handle_sys_interrupt_trigger(handle: hid_t) -> SyscallResult {
    let thread = CPULocalStorageRW::get_current_task();

    let handle = kunwrap2!(Hid::from_raw(handle));
    let int = kunwrap2!(thread.process().get_value(handle));
    let int = kenum_cast2!(int, KernelValue::Interrupt);
    int.trigger();
    SyscallResult::Ok
}

unsafe extern "C" fn handle_sys_interrupt_acknowledge(handle: hid_t) -> SyscallResult {
    let thread = CPULocalStorageRW::get_current_task();

    let handle = kunwrap2!(Hid::from_raw(handle));
    let int = kunwrap2!(thread.process().get_value(handle));
    let int = kenum_cast2!(int, KernelValue::Interrupt);
    int.ack();
    SyscallResult::Ok
}

unsafe extern "C" fn handle_sys_interrupt_set_port(
    handle: hid_t,
    port: hid_t,
    key: u64,
) -> SyscallResult {
    let thread = CPULocalStorageRW::get_current_task();

    let int = kunwrap2!(Hid::from_raw(handle));
    let int = kunwrap2!(thread.process().get_value(int));
    let int = kenum_cast2!(int, KernelValue::Interrupt);

    let port = kunwrap2!(Hid::from_raw(port));
    let port = kunwrap2!(thread.process().get_value(port));
    let port = kenum_cast2!(port, KernelValue::Port);
    int.set_port(port, key);
    SyscallResult::Ok
}

// port

unsafe extern "C" fn handle_sys_port_create() -> hid_t {
    let thread = CPULocalStorageRW::get_current_task();

    let interrupt = KPort::new();
    let id = thread.process().add_value(Arc::new(interrupt).into());
    id.0.get()
}

unsafe extern "C" fn handle_sys_port_wait(
    handle: hid_t,
    result: *mut sys_port_notification_t,
) -> SyscallResult {
    let thread = CPULocalStorageRW::get_current_task();

    let handle = kunwrap2!(Hid::from_raw(handle));
    let handle = kunwrap2!(thread.process().get_value(handle));
    let port = kenum_cast2!(handle, KernelValue::Port);

    *result = port.wait().into_raw();

    SyscallResult::Ok
}

unsafe extern "C" fn handle_sys_port_push(
    handle: hid_t,
    value: *const sys_port_notification_t,
) -> SyscallResult {
    let thread = CPULocalStorageRW::get_current_task();

    let handle = kunwrap2!(Hid::from_raw(handle));
    let handle = kunwrap2!(thread.process().get_value(handle));
    let port = kenum_cast2!(handle, KernelValue::Port);

    let value = kunwrap2!(SysPortNotification::from_raw(*value));

    port.notify(value);

    SyscallResult::Ok
}

// process

unsafe extern "C" fn handle_sys_process_spawn_thread(func: *const (), arg: *const ()) -> tid_t {
    taskmanager::spawn_thread(func as usize, arg as usize).into_raw()
}

unsafe extern "C" fn handle_sys_process_exit_code(
    handle: hid_t,
    exit: *mut usize,
) -> SyscallResult {
    let thread = CPULocalStorageRW::get_current_task();

    let handle = kunwrap2!(Hid::from_raw(handle));
    let handle = kunwrap2!(thread.process().get_value(handle));
    let proc = kenum_cast2!(handle, KernelValue::Process);
    let status = *proc.exit_status.lock();
    match status {
        Some(val) => {
            *exit = val;
            SyscallResult::Ok
        }
        None => SyscallResult::ProcessStillRunning,
    }
}

// message

unsafe extern "C" fn handle_sys_message_create(data: *const u8, len: usize) -> hid_t {
    let thread = CPULocalStorageRW::get_current_task();

    let data: Box<[u8]> = core::slice::from_raw_parts(data, len).into();
    let msg = Arc::new(KMessage { data });

    thread.process().add_value(msg.into()).into_raw()
}

unsafe extern "C" fn handle_sys_message_size(handle: hid_t, size: *mut usize) -> SyscallResult {
    let thread = CPULocalStorageRW::get_current_task();
    let handle = kunwrap2!(Hid::from_raw(handle));
    let handle = kunwrap2!(thread.process().get_value(handle));
    let message = kenum_cast2!(handle, KernelValue::Message);
    *size = message.data.len();
    SyscallResult::Ok
}

unsafe extern "C" fn handle_sys_message_read(
    handle: hid_t,
    buffer: *mut u8,
    buf_len: usize,
) -> SyscallResult {
    let thread = CPULocalStorageRW::get_current_task();
    let handle = kunwrap2!(Hid::from_raw(handle));
    let handle = kunwrap2!(thread.process().get_value(handle));
    let message = kenum_cast2!(handle, KernelValue::Message);

    kassert2!(
        message.data.len() == buf_len,
        "Data and loc len should be same instead was: {} {}",
        message.data.len(),
        buf_len
    );

    let loc = core::slice::from_raw_parts_mut(buffer, buf_len);
    loc.copy_from_slice(&message.data);
    SyscallResult::Ok
}
