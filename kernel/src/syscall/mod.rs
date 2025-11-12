use alloc::{sync::Arc, vec::Vec};
use kernel_sys::{
    raw::{syscall::*, types::result_t},
    types::{
        Hid, ObjectSignal, RawValue, SysPortNotification, SysPortNotificationValue, SyscallError,
        SyscallResult, VMMapFlags, VMOAnonymousFlags,
    },
};
use log::Level;
use x86_64::structures::idt::{InterruptDescriptorTable, InterruptStackFrame};

use crate::{
    channel::{ChannelMessage, ReadError, channel_create},
    cpu_localstorage::{CPULocalStorage, CPULocalStorageRW},
    interrupts::KInterruptHandle,
    logging::print_log,
    message::KMessage,
    mutex::Spinlock,
    object::{KObject, KObjectSignal, SignalWaiter},
    port::KPort,
    scheduling::{
        process::{
            KernelValue, ProcessMemory, ProcessPrivilege, ProcessReferences, Thread, ThreadState,
        },
        taskmanager::{self, enter_sched, kill_bad_task},
    },
    time::{SLEPT_PROCESSES, SleptProcess, uptime},
    user::{UserBytes, UserBytesMut, UserPtr, UserPtrBounds, UserPtrMut, get_current_bounds},
    vm::VMO,
};

pub fn set_syscall_idt(idt: &mut InterruptDescriptorTable) {
    idt[SYSCALL_NUMBER]
        .set_handler_fn(wrapped_syscall_handler)
        .set_privilege_level(x86_64::PrivilegeLevel::Ring3);

    // .disable_interrupts(false);
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
            return Err(kernel_sys::types::SyscallError::KernelPrivateFailAssertion);
        }
    };
}

#[macro_export]
macro_rules! kassert {
    ($x: expr) => {
        if !$x {
            error!("KAssert failed in {}:{}:{}.", file!(), line!(), column!());
            return Err(kernel_sys::types::SyscallError::KernelPrivateFailAssertion);
        }
    };
    ($x: expr, $($arg:tt)+) => {
        if !$x {
            error!("KAssert failed in {}:{}:{} {}", file!(), line!(), column!(), format_args!($($arg)*));
            return Err(kernel_sys::types::SyscallError::KernelPrivateFailAssertion);
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
                return Err(kernel_sys::types::SyscallError::KernelPrivateFailAssertion);
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
                return Err(SyscallError::KernelPrivateFailAssertion);
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
        "cmp qword ptr gs:{int_depth}, 0",
        "jne {bad_interrupts_held}",

        // save regs
        "push rbp",
        "push r15",
        "pushfq",
        "cli",

        // set cpu context
        "mov r11b, 1",
        "mov gs:{ctx}, r11b",
        "mov r15, rsp",     // save caller rsp
        "mov rsp, gs:{kstack}", // load kstack top
        "sti",

        // args
        "push r9",
        "push r8",
        "push rcx",
        "push rdx",
        "push rsi",
        "push rdi",

        // number
        "mov rdi, rax",
        "mov rsi, rsp",

        "call {handler}",

        "add rsp, 8*6",

        // set cpu context
        "cli",
        "mov cl, 2",
        "mov gs:{ctx}, cl", // set cpu context

        // restore regs
        "mov rsp, r15",   // restore caller rip
        "popfq",
        "pop r15",
        "pop rbp",
        "ret",
        handler = sym syscall_handler,
        bad_interrupts_held = sym bad_interrupts_held,
        int_depth = const core::mem::offset_of!(CPULocalStorage, hold_interrupts_depth),
        ctx = const core::mem::offset_of!(CPULocalStorage, current_context),
        kstack = const core::mem::offset_of!(CPULocalStorage, current_task_kernel_stack_top),
    );
}

/// Handler for syscalls via int 0x80
#[unsafe(naked)]
extern "x86-interrupt" fn wrapped_syscall_handler(_: InterruptStackFrame) {
    core::arch::naked_asm!(
        // set cpu context
        "mov r11b, 1",
        "mov gs:{ctx}, r11b",
        "sti",

        // args
        "push r9",
        "push r8",
        "push rcx",
        "push rdx",
        "push rsi",
        "push rdi",

        // number
        "mov rdi, rax",
        "mov rsi, rsp",

        "call {handler}",

        "add rsp, 8*6",

        // set cpu context
        "cli",
        "mov cl, 2",
        "mov gs:{ctx}, cl",

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
        handler = sym syscall_handler,
        ctx = const core::mem::offset_of!(CPULocalStorage, current_context),
    );
}

/// Handler for syscalls via syscall
#[unsafe(naked)]
pub unsafe extern "C" fn syscall_sysret_handler() {
    core::arch::naked_asm!(
        // set cpu context
        "mov r12d, 1",
        "mov gs:{ctx}, r12d",

        // swap stack
        "mov r12, rsp",
        "mov rsp, gs:{kstack}",
        "sti",

        // save registers
        "push r11", // save caller flags
        "push rcx", // save caller rip

        // args
        "push r9",
        "push r8",
        "push r10", // not in rcx because syscall
        "push rdx",
        "push rsi",
        "push rdi",

        // number
        "mov rdi, rax",
        "mov rsi, rsp",

        "call {handler}",

        "add rsp, 8*6",

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
        "mov gs:{ctx}, cl",

        // restore registers
        "pop rcx",
        "pop r11",
        "mov rsp, r12",
        "sysretq",
        handler = sym syscall_handler,
        ctx = const core::mem::offset_of!(CPULocalStorage, current_context),
        kstack = const core::mem::offset_of!(CPULocalStorage, current_task_kernel_stack_top),
    );
}

pub struct SyscallContext<'a> {
    thread: &'a Thread,
    bounds: UserPtrBounds,
}

extern "C" fn syscall_handler(number: usize, args: &[usize; 6]) -> result_t {
    let Some(syscalls) = parse_syscalls(number, args) else {
        info!("Out of bounds syscall {number}");
        return Err(SyscallError::UnknownSyscall).into_raw();
    };

    let mut ctx = unsafe {
        let thread = CPULocalStorageRW::get_current_task();
        SyscallContext {
            thread,
            bounds: get_current_bounds(thread.process()),
        }
    };

    match ctx.dispatch(&syscalls) {
        Err(SyscallError::KernelPrivateFailAssertion) => kill_bad_task(),
        r => r.into_raw(),
    }
}

impl DispatchSyscall for SyscallContext<'_> {
    fn raw_sys_echo(&mut self, req: &RawSysEcho) -> SyscallResult {
        let mut res = unsafe { kunwrap!(UserPtrMut::new(req.val2, self.bounds)) };
        info!("ECHO {}", req.val);
        res.write(|| req.val);

        Ok(())
    }

    fn raw_sys_yield(&mut self, _req: &RawSysYield) -> SyscallResult {
        let mut sched = self.thread.sched().lock();
        enter_sched(&mut sched);
        Ok(())
    }

    fn raw_sys_sleep(&mut self, req: &RawSysSleep) -> SyscallResult {
        let mut slept = unsafe { kunwrap!(UserPtrMut::new(req.slept, self.bounds)) };

        let start = uptime();
        let time = start + req.ms;

        let mut sched = self.thread.sched().lock();
        sched.state = ThreadState::Sleeping;

        SLEPT_PROCESSES
            .lock()
            .push(core::cmp::Reverse(SleptProcess {
                wakeup: time,
                thread: self.thread.thread(),
            }));

        enter_sched(&mut sched);
        slept.write(|| uptime() - start);
        Ok(())
    }

    fn raw_sys_exit(&mut self, _req: &RawSysExit) -> SyscallResult {
        let mut sched = self.thread.sched().lock();
        sched.state = ThreadState::Killed;
        enter_sched(&mut sched);
        unreachable!("exit thread shouldn't return")
    }

    fn raw_sys_map(&mut self, req: &RawSysMap) -> SyscallResult {
        let mut result = kunwrap!(unsafe { UserPtrMut::new(req.result, self.bounds) });

        kassert!(req.hint as usize + req.length <= self.bounds.top());

        let memory: &mut ProcessMemory = &mut self.thread.process().memory.lock();
        let refs: &mut ProcessReferences = &mut self.thread.process().references.lock();

        let flags = VMMapFlags::from_bits_truncate(req.flags);

        let vmo_handle = match Hid::from_raw(req.vmo) {
            Ok(vmo) => {
                let val = kunwrap!(refs.get(vmo));
                kenum_cast!(val, KernelValue::VMO).clone()
            }
            Err(_) => {
                // allocate anonymous object for the mapping
                Arc::new(Spinlock::new(VMO::new_anonymous(
                    req.length,
                    VMOAnonymousFlags::empty(),
                )))
            }
        };

        let hint = (!req.hint.is_null()).then_some(req.hint as usize);
        match memory.region.map_vmo(vmo_handle, flags, hint) {
            Ok(res) => result.write(|| res as *mut ()),
            Err(e) => {
                error!("Err {e:?}");
                return Err(SyscallError::BadInputPointer);
            }
        }

        Ok(())
    }

    fn raw_sys_unmap(&mut self, req: &RawSysUnmap) -> SyscallResult {
        let RawSysUnmap { address, length } = *req;

        kassert!(address as usize + length <= self.bounds.top());

        let memory: &mut ProcessMemory = &mut self.thread.process().memory.lock();

        match unsafe { memory.region.unmap(req.address as usize, length) } {
            Ok(()) => Ok(()),
            Err(err) => {
                info!("Error unmapping: {address:?}-{length} {err:?}");
                Err(SyscallError::BadInputPointer)
            }
        }
    }

    fn raw_sys_read_args(&mut self, req: &RawSysReadArgs) -> SyscallResult {
        let RawSysReadArgs {
            buffer,
            len,
            out_len,
        } = *req;
        let mut result = unsafe { kunwrap!(UserBytesMut::new(buffer, len, self.bounds)) };
        let mut out_len = unsafe { kunwrap!(UserPtrMut::new(out_len, self.bounds)) };

        let proc = self.thread.process();
        let bytes = &proc.args;

        out_len.write(|| bytes.len());

        if buffer.is_null() {
            return Ok(());
        }

        if len != bytes.len() {
            return Err(SyscallError::BadInputPointer);
        }

        result.write(bytes);

        Ok(())
    }

    fn raw_sys_pid(&mut self, req: &RawSysPid) -> SyscallResult {
        let mut pid = unsafe { kunwrap!(UserPtrMut::new(req.pid, self.bounds)) };
        pid.write(|| self.thread.process().pid.into_raw());
        Ok(())
    }

    fn raw_sys_log(&mut self, req: &RawSysLog) -> SyscallResult {
        unsafe {
            let target = kunwrap!(UserBytes::new(req.target, req.target_len, self.bounds));
            let message = kunwrap!(UserBytes::new(req.message, req.message_len, self.bounds));

            let target = target.read_to_box();
            let message = message.read_to_box();

            let target = kunwrap!(core::str::from_utf8(&target));
            let message = kunwrap!(core::str::from_utf8(&message));
            let level = match req.level {
                1 => Level::Error,
                2 => Level::Warn,
                3 => Level::Info,
                4 => Level::Debug,
                5 => Level::Trace,
                level => {
                    kpanic!("Invalid level {level}");
                }
            };

            print_log(level, target, &format_args!("{message}"));
        }

        Ok(())
    }

    fn raw_sys_handle_drop(&mut self, req: &RawSysHandleDrop) -> SyscallResult {
        let handle = kunwrap!(Hid::from_raw(req.handle));

        let refs: &mut ProcessReferences = &mut self.thread.process().references.lock();

        match refs.remove(handle) {
            Some(_) => Ok(()),
            None => Err(SyscallError::UnknownHandle),
        }
    }

    fn raw_sys_handle_clone(&mut self, req: &RawSysHandleClone) -> SyscallResult {
        let mut cloned = unsafe { kunwrap!(UserPtrMut::new(req.cloned, self.bounds)) };
        let handle = kunwrap!(Hid::from_raw(req.handle));

        let refs: &mut ProcessReferences = &mut self.thread.process().references.lock();

        match refs.get(handle).cloned() {
            Some(h) => {
                let new = refs.insert(h);
                cloned.write(|| new.0.get());
                Ok(())
            }
            None => Err(SyscallError::UnknownHandle),
        }
    }

    fn raw_sys_object_type(&mut self, req: &RawSysObjectType) -> SyscallResult {
        let mut ty = unsafe { kunwrap!(UserPtrMut::new(req.ty, self.bounds)) };
        let handle = kunwrap!(Hid::from_raw(req.handle));

        let refs: &mut ProcessReferences = &mut self.thread.process().references.lock();

        match refs.get(handle) {
            Some(h) => {
                ty.write(|| h.object_type() as usize);
                Ok(())
            }
            None => Err(SyscallError::UnknownHandle),
        }
    }

    fn raw_sys_object_wait(&mut self, req: &RawSysObjectWait) -> SyscallResult {
        let mut result = unsafe { kunwrap!(UserPtrMut::new(req.result, self.bounds)) };
        let handle = kunwrap!(Hid::from_raw(req.handle));

        let refs = self.thread.process().references.lock();

        let Some(val) = refs.get(handle).cloned() else {
            return Err(SyscallError::UnknownHandle);
        };

        let mask = ObjectSignal::from_bits_truncate(req.on);

        let waiter = |signals: &mut KObjectSignal| {
            if signals.signal_status().intersects(mask) {
                Ok(signals.signal_status())
            } else {
                let mut sched = self.thread.sched().lock();
                sched.state = ThreadState::Sleeping;
                signals.wait(SignalWaiter {
                    ty: crate::object::SignalWaiterType::One(self.thread.thread()),
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
            Ok(val) => result.write(|| val.bits()),
            Err(mut status) => {
                drop(refs);
                enter_sched(&mut status);
                result.write(|| match val {
                    KernelValue::Channel(v) => v.signals(|w| w.signal_status().bits()),
                    KernelValue::Process(v) => v.signals(|w| w.signal_status().bits()),
                    _ => panic!("object not signalable"),
                })
            }
        }
        Ok(())
    }

    fn raw_sys_object_wait_port(&mut self, req: &RawSysObjectWaitPort) -> SyscallResult {
        let handle = kunwrap!(Hid::from_raw(req.handle));
        let port = kunwrap!(Hid::from_raw(req.port));

        let refs = self.thread.process().references.lock();

        let handle = refs.get(handle).ok_or(SyscallError::UnknownHandle)?;
        let port = refs.get(port).ok_or(SyscallError::UnknownHandle)?;

        let mask = ObjectSignal::from_bits_truncate(req.mask);

        let port = kenum_cast!(port, KernelValue::Port);

        let waiter = |signals: &mut KObjectSignal| {
            if signals.signal_status().intersects(mask) {
                port.notify(SysPortNotification {
                    key: req.key,
                    value: SysPortNotificationValue::SignalOne {
                        trigger: mask,
                        signals: signals.signal_status(),
                    },
                });
            } else {
                signals.wait(SignalWaiter {
                    ty: crate::object::SignalWaiterType::Port {
                        port: port.clone(),
                        key: req.key,
                    },
                    mask,
                });
            }
        };

        match &handle {
            KernelValue::Channel(v) => v.signals(waiter),
            KernelValue::Process(v) => v.signals(waiter),
            _ => kpanic!("object not signalable"),
        };

        Ok(())
    }

    fn raw_sys_channel_create(&mut self, req: &RawSysChannelCreate) -> SyscallResult {
        let mut left = unsafe { kunwrap!(UserPtrMut::new(req.left, self.bounds)) };
        let mut right = unsafe { kunwrap!(UserPtrMut::new(req.right, self.bounds)) };

        kassert!(!left.is_null() && !right.is_null());

        let (l, r) = channel_create();

        let l = self.thread.process().add_value(l.into());
        let r = self.thread.process().add_value(r.into());

        left.write(|| l.into_raw());
        right.write(|| r.into_raw());

        Ok(())
    }

    fn raw_sys_channel_read(&mut self, req: &RawSysChannelRead) -> SyscallResult {
        let mut data_len = unsafe { kunwrap!(UserPtrMut::new(req.data_len, self.bounds)) };
        let mut handles_len = unsafe { kunwrap!(UserPtrMut::new(req.handles_len, self.bounds)) };

        let handle = kunwrap!(Hid::from_raw(req.handle));
        let handle = kunwrap!(self.thread.process().get_value(handle));
        let chan = kenum_cast!(handle, KernelValue::Channel);

        match chan.read(kunwrap!(data_len.read()), kunwrap!(handles_len.read())) {
            Ok(ok) => {
                data_len.write(|| ok.data.len());
                let mut data_buf =
                    unsafe { kunwrap!(UserBytesMut::new(req.data, ok.data.len(), self.bounds)) };

                data_buf.write(&ok.data);

                if let Some(h) = ok.handles {
                    handles_len.write(|| h.len());

                    let mut handle_ptr = req.handles;
                    for handle in h {
                        let id = self.thread.process().add_value(handle);
                        let mut h = unsafe { kunwrap!(UserPtrMut::new(handle_ptr, self.bounds)) };
                        h.write(|| id.into_raw());
                        if !handle_ptr.is_null() {
                            handle_ptr = handle_ptr.wrapping_add(1)
                        };
                    }
                } else {
                    handles_len.write(|| 0);
                }
                Ok(())
            }
            Err(ReadError::Empty) => Err(SyscallError::ChannelEmpty),
            Err(ReadError::Size {
                min_bytes,
                min_handles,
            }) => {
                data_len.write(|| min_bytes);
                handles_len.write(|| min_handles);
                Err(SyscallError::ChannelBufferTooSmall)
            }
            Err(ReadError::Closed) => Err(SyscallError::ChannelClosed),
        }
    }

    fn raw_sys_channel_write(&mut self, req: &RawSysChannelWrite) -> SyscallResult {
        let data = unsafe { kunwrap!(UserBytes::new(req.data, req.data_len, self.bounds)) };

        let handle = kunwrap!(Hid::from_raw(req.handle));
        let handle = kunwrap!(self.thread.process().get_value(handle));
        let chan = kenum_cast!(handle, KernelValue::Channel);

        let handles = if !req.handles.is_null() && req.handles_len > 0 {
            let mut handles_res = Vec::with_capacity(req.handles_len);
            let refs = self.thread.process().references.lock();
            for i in 0..req.handles_len {
                let h = unsafe { kunwrap!(UserPtr::new(req.handles.wrapping_add(i), self.bounds)) };
                let r = kunwrap!(Hid::from_raw(h.read().expect("shouldn't be null")));
                handles_res.push(kunwrap!(refs.get(r)).clone());
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

    fn raw_sys_interrupt_create(&mut self, req: &RawSysInterruptCreate) -> SyscallResult {
        let mut data = unsafe { kunwrap!(UserPtrMut::new(req.out, self.bounds)) };

        let interrupt = KInterruptHandle::new();
        let id = self.thread.process().add_value(Arc::new(interrupt).into());
        data.write(|| id.0.get());
        Ok(())
    }

    fn raw_sys_interrupt_wait(&mut self, req: &RawSysInterruptWait) -> SyscallResult {
        let handle = kunwrap!(Hid::from_raw(req.handle));
        let int = kunwrap!(self.thread.process().get_value(handle));
        let int = kenum_cast!(int, KernelValue::Interrupt);
        int.wait()
    }

    fn raw_sys_interrupt_trigger(&mut self, req: &RawSysInterruptTrigger) -> SyscallResult {
        let handle = kunwrap!(Hid::from_raw(req.handle));
        let int = kunwrap!(self.thread.process().get_value(handle));
        let int = kenum_cast!(int, KernelValue::Interrupt);
        int.trigger();
        Ok(())
    }

    fn raw_sys_interrupt_acknowledge(&mut self, req: &RawSysInterruptAcknowledge) -> SyscallResult {
        let handle = kunwrap!(Hid::from_raw(req.handle));
        let int = kunwrap!(self.thread.process().get_value(handle));
        let int = kenum_cast!(int, KernelValue::Interrupt);
        int.ack();
        Ok(())
    }

    fn raw_sys_interrupt_set_port(&mut self, req: &RawSysInterruptSetPort) -> SyscallResult {
        let int = kunwrap!(Hid::from_raw(req.handle));
        let int = kunwrap!(self.thread.process().get_value(int));
        let int = kenum_cast!(int, KernelValue::Interrupt);

        let port = kunwrap!(Hid::from_raw(req.port));
        let port = kunwrap!(self.thread.process().get_value(port));
        let port = kenum_cast!(port, KernelValue::Port);
        int.set_port(port, req.key);
        Ok(())
    }

    fn raw_sys_port_create(&mut self, req: &RawSysPortCreate) -> SyscallResult {
        let mut data = unsafe { kunwrap!(UserPtrMut::new(req.out, self.bounds)) };

        let interrupt = KPort::new();
        let id = self.thread.process().add_value(Arc::new(interrupt).into());
        data.write(|| id.0.get());
        Ok(())
    }

    fn raw_sys_port_wait(&mut self, req: &RawSysPortWait) -> SyscallResult {
        let mut result = unsafe { kunwrap!(UserPtrMut::new(req.result, self.bounds)) };

        let handle = kunwrap!(Hid::from_raw(req.handle));
        let handle = kunwrap!(self.thread.process().get_value(handle));
        let port = kenum_cast!(handle, KernelValue::Port);

        let w = port.wait();
        result.write(|| w.into_raw());

        Ok(())
    }

    fn raw_sys_port_push(&mut self, req: &RawSysPortPush) -> SyscallResult {
        let value = unsafe { kunwrap!(UserPtr::new(req.notification, self.bounds)) };
        kassert!(!value.is_null());

        let handle = kunwrap!(Hid::from_raw(req.handle));
        let handle = kunwrap!(self.thread.process().get_value(handle));
        let port = kenum_cast!(handle, KernelValue::Port);

        let value = kunwrap!(SysPortNotification::from_raw(
            value.read().expect("should be non null")
        ));

        port.notify(value);
        Ok(())
    }

    fn raw_sys_process_spawn_thread(&mut self, req: &RawSysProcessSpawnThread) -> SyscallResult {
        unsafe {
            let mut out = kunwrap!(UserPtrMut::new(req.out, self.bounds));
            let tid = taskmanager::spawn_thread(req.func as usize, req.arg as usize);

            out.write(|| tid.into_raw());
        }
        Ok(())
    }

    fn raw_sys_process_exit_code(&mut self, req: &RawSysProcessExitCode) -> SyscallResult {
        let mut exit = unsafe { kunwrap!(UserPtrMut::new(req.exit, self.bounds)) };

        let handle = kunwrap!(Hid::from_raw(req.handle));
        let handle = kunwrap!(self.thread.process().get_value(handle));
        let proc = kenum_cast!(handle, KernelValue::Process);
        let status = *proc.exit_status.lock();
        match status {
            Some(val) => {
                exit.write(|| val);
                Ok(())
            }
            None => Err(SyscallError::ProcessStillRunning),
        }
    }

    fn raw_sys_message_create(&mut self, req: &RawSysMessageCreate) -> SyscallResult {
        let data = unsafe { kunwrap!(UserBytes::new(req.data, req.data_len, self.bounds)) };
        let mut out = unsafe { kunwrap!(UserPtrMut::new(req.out, self.bounds)) };

        let msg = Arc::new(KMessage {
            data: data.read_to_box(),
        });

        let res = self.thread.process().add_value(msg.into());

        out.write(|| res.into_raw());
        Ok(())
    }

    fn raw_sys_message_size(&mut self, req: &RawSysMessageSize) -> SyscallResult {
        let mut size = unsafe { kunwrap!(UserPtrMut::new(req.size, self.bounds)) };

        let handle = kunwrap!(Hid::from_raw(req.handle));
        let handle = kunwrap!(self.thread.process().get_value(handle));
        let message = kenum_cast!(handle, KernelValue::Message);
        size.write(|| message.data.len());
        Ok(())
    }

    fn raw_sys_message_read(&mut self, req: &RawSysMessageRead) -> SyscallResult {
        let mut buffer = unsafe { kunwrap!(UserBytesMut::new(req.buf, req.buf_len, self.bounds)) };

        let handle = kunwrap!(Hid::from_raw(req.handle));
        let handle = kunwrap!(self.thread.process().get_value(handle));
        let message = kenum_cast!(handle, KernelValue::Message);

        kassert!(
            message.data.len() == req.buf_len,
            "Data and loc len should be same instead was: {} {}",
            message.data.len(),
            req.buf_len
        );

        buffer.write(&message.data);

        Ok(())
    }

    fn raw_sys_vmo_mmap_create(&mut self, req: &RawSysVMOMMAPCreate) -> SyscallResult {
        unsafe {
            kassert!(
                self.thread.process().privilege == ProcessPrivilege::KERNEL,
                "MMAP is privileged"
            );

            let mut out = kunwrap!(UserPtrMut::new(req.out, self.bounds));

            let vmo = Arc::new(Spinlock::new(VMO::new_mmap(req.base as usize, req.length)));
            let res = self.thread.process().references.lock().insert(vmo.into());
            out.write(|| res.into_raw());
        }
        Ok(())
    }

    fn raw_sys_vmo_anonymous_create(&mut self, req: &RawSysVMOAnonCreate) -> SyscallResult {
        let mut out = unsafe { kunwrap!(UserPtrMut::new(req.out, self.bounds)) };

        let flags = VMOAnonymousFlags::from_bits_truncate(req.flags);

        if flags.intersects(VMOAnonymousFlags::_PRIVILEGED)
            && self.thread.process().privilege == ProcessPrivilege::USER
        {
            kpanic!("Only kernel can use privileged flags");
        }

        let vmo = Arc::new(Spinlock::new(VMO::new_anonymous(req.length, flags)));
        let res = self.thread.process().references.lock().insert(vmo.into());
        out.write(|| res.into_raw());

        Ok(())
    }

    fn raw_sys_vmo_anonymous_pinned_addresses(
        &mut self,
        req: &RawSysVMOAnonPinned,
    ) -> SyscallResult {
        if self.thread.process().privilege == ProcessPrivilege::USER {
            kpanic!("Only kernel can use");
        }

        let handle = kunwrap!(Hid::from_raw(req.handle));

        let refs = self.thread.process().references.lock();
        let val = kunwrap!(refs.get(handle));
        let vmo = kenum_cast!(val, KernelValue::VMO);
        match &*vmo.lock() {
            VMO::MemoryMapped { .. } => kpanic!("not anonymous"),
            VMO::Anonymous { flags, pages } => {
                kassert!(flags.contains(VMOAnonymousFlags::PINNED));

                for (i, p) in pages.iter().skip(req.offset).enumerate().take(req.length) {
                    let mut ptr =
                        unsafe { kunwrap!(UserPtrMut::new(req.result.add(i), self.bounds)) };
                    ptr.write(|| p.map(|v| v.get_address() as usize).unwrap_or(0));
                }
            }
        }

        Ok(())
    }
}
