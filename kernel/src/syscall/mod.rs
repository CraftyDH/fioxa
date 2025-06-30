pub mod exit_userspace;

use core::ops::ControlFlow;

use alloc::{sync::Arc, vec::Vec};
use kernel_sys::{
    raw::{
        syscall::{KernelSyscallHandlerBreak, SYSCALL_NUMBER, SyscallHandler},
        types::{hid_t, pid_t, signals_t, sys_port_notification_t, tid_t, vaddr_t},
    },
    types::{
        Hid, ObjectSignal, RawValue, SysPortNotification, SysPortNotificationValue, SyscallResult,
        VMMapFlags, VMOAnonymousFlags,
    },
};
use log::Level;
use x86_64::structures::idt::InterruptDescriptorTable;

use crate::{
    channel::{ChannelMessage, ReadError, channel_create},
    gdt::VM_HANDLER_FUNCS_IST_INDEX,
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
        taskmanager::{self, enter_sched},
    },
    syscall::exit_userspace::wrapped_syscall_handler,
    time::{SLEPT_PROCESSES, SleptProcess, uptime},
    user::{UserBytes, UserBytesMut, UserPtr, UserPtrMut, get_current_bounds},
    vm::VMO,
};

pub fn set_syscall_idt(idt: &mut InterruptDescriptorTable) {
    unsafe {
        idt[SYSCALL_NUMBER]
            .set_handler_fn(wrapped_syscall_handler)
            .set_stack_index(VM_HANDLER_FUNCS_IST_INDEX)
            .set_privilege_level(x86_64::PrivilegeLevel::Ring3);
    }
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
            return ControlFlow::Break(KernelSyscallHandlerBreak::AssertFailed);
        }
    };
}

#[macro_export]
macro_rules! kassert {
    ($x: expr) => {
        if !$x {
            error!("KAssert failed in {}:{}:{}.", file!(), line!(), column!());
            return ControlFlow::Break(KernelSyscallHandlerBreak::AssertFailed);
        }
    };
    ($x: expr, $($arg:tt)+) => {
        if !$x {
            error!("KAssert failed in {}:{}:{} {}", file!(), line!(), column!(), format_args!($($arg)*));
            return ControlFlow::Break(KernelSyscallHandlerBreak::AssertFailed);
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
                return ControlFlow::Break(KernelSyscallHandlerBreak::AssertFailed);
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
                return ControlFlow::Break(KernelSyscallHandlerBreak::AssertFailed);
            }
        }
    };
}

use kernel_sys::raw::types::*;

pub struct KernelSyscallHandler<'a> {
    pub thread: &'a Thread,
}

impl SyscallHandler for KernelSyscallHandler<'_> {
    fn raw_sys_echo(
        &mut self,
        val: usize,
    ) -> core::ops::ControlFlow<KernelSyscallHandlerBreak, usize> {
        info!("ECHO {val}");
        ControlFlow::Continue(val)
    }

    fn raw_sys_yield(&mut self) -> core::ops::ControlFlow<KernelSyscallHandlerBreak> {
        let mut sched = self.thread.sched().lock();
        enter_sched(&mut sched);
        ControlFlow::Continue(())
    }

    fn raw_sys_sleep(&mut self, ms: u64) -> core::ops::ControlFlow<KernelSyscallHandlerBreak, u64> {
        let start = uptime();
        let time = start + ms;

        let mut sched = self.thread.sched().lock();
        sched.state = ThreadState::Sleeping;

        SLEPT_PROCESSES
            .lock()
            .push(core::cmp::Reverse(SleptProcess {
                wakeup: time,
                thread: self.thread.thread(),
            }));

        enter_sched(&mut sched);
        ControlFlow::Continue(uptime() - start)
    }

    fn raw_sys_exit(&mut self) -> core::ops::ControlFlow<KernelSyscallHandlerBreak, !> {
        ControlFlow::Break(KernelSyscallHandlerBreak::Exit)
    }

    fn raw_sys_map(
        &mut self,
        vmo: hid_t,
        flags: u32,
        hint: vaddr_t,
        length: usize,
        result: *mut vaddr_t,
    ) -> core::ops::ControlFlow<KernelSyscallHandlerBreak, result_t> {
        let bounds = get_current_bounds(&self.thread.process());
        kassert!(hint as usize + length <= bounds.top());
        let mut result = kunwrap!(unsafe { UserPtrMut::new(result, bounds) });

        let memory: &mut ProcessMemory = &mut self.thread.process().memory.lock();
        let refs: &mut ProcessReferences = &mut self.thread.process().references.lock();

        let flags = VMMapFlags::from_bits_truncate(flags);

        let vmo_handle = match Hid::from_raw(vmo) {
            Ok(vmo) => {
                let val = kunwrap!(refs.get(vmo));
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
                return ControlFlow::Continue(SyscallResult::BadInputPointer.into_raw());
            }
        }
        ControlFlow::Continue(SyscallResult::Ok.into_raw())
    }

    fn raw_sys_unmap(
        &mut self,
        address: vaddr_t,
        length: usize,
    ) -> core::ops::ControlFlow<KernelSyscallHandlerBreak, result_t> {
        let bounds = get_current_bounds(self.thread.process());
        kassert!(address as usize + length <= bounds.top());

        let memory: &mut ProcessMemory = &mut self.thread.process().memory.lock();

        let res = match unsafe { memory.region.unmap(address as usize, length) } {
            Ok(()) => SyscallResult::Ok,
            Err(err) => {
                info!("Error unmapping: {address:?}-{length} {err:?}");
                SyscallResult::BadInputPointer
            }
        };

        ControlFlow::Continue(res.into_raw())
    }

    fn raw_sys_read_args(
        &mut self,
        buffer: *mut u8,
        len: usize,
    ) -> core::ops::ControlFlow<KernelSyscallHandlerBreak, usize> {
        let bounds = get_current_bounds(self.thread.process());

        let mut result = unsafe { kunwrap!(UserBytesMut::new(buffer, len, bounds)) };

        let proc = self.thread.process();
        let bytes = &proc.args;

        if buffer.is_null() || len != bytes.len() {
            return ControlFlow::Continue(bytes.len());
        }

        result.write(&bytes);

        ControlFlow::Continue(usize::MAX)
    }

    fn raw_sys_pid(&mut self) -> core::ops::ControlFlow<KernelSyscallHandlerBreak, pid_t> {
        ControlFlow::Continue(self.thread.process().pid.into_raw())
    }

    fn raw_sys_log(
        &mut self,
        level: u32,
        target: *const u8,
        target_len: usize,
        message: *const u8,
        message_len: usize,
    ) -> core::ops::ControlFlow<KernelSyscallHandlerBreak> {
        unsafe {
            let bounds = get_current_bounds(&self.thread.process());

            let target = kunwrap!(UserBytes::new(target, target_len, bounds));
            let message = kunwrap!(UserBytes::new(message, message_len, bounds));

            let target = target.read_to_box();
            let message = message.read_to_box();

            let target = kunwrap!(core::str::from_utf8(&target));
            let message = kunwrap!(core::str::from_utf8(&message));

            let level = match level {
                1 => Level::Error,
                2 => Level::Warn,
                3 => Level::Info,
                4 => Level::Debug,
                5 => Level::Trace,
                _ => {
                    kpanic!("Invalid level {level}")
                }
            };

            print_log(level, target, &format_args!("{message}"));
            ControlFlow::Continue(())
        }
    }

    fn raw_sys_handle_drop(
        &mut self,
        handle: hid_t,
    ) -> core::ops::ControlFlow<KernelSyscallHandlerBreak, result_t> {
        let refs: &mut ProcessReferences = &mut self.thread.process().references.lock();

        let handle = kunwrap!(Hid::from_raw(handle));

        let res = match refs.remove(handle) {
            Some(_) => SyscallResult::Ok,
            None => SyscallResult::UnknownHandle,
        };
        ControlFlow::Continue(res.into_raw())
    }

    fn raw_sys_handle_clone(
        &mut self,
        handle: hid_t,
        cloned: *mut hid_t,
    ) -> core::ops::ControlFlow<KernelSyscallHandlerBreak, result_t> {
        let bounds = get_current_bounds(&self.thread.process());
        let mut cloned = unsafe { kunwrap!(UserPtrMut::new(cloned, bounds)) };

        let refs: &mut ProcessReferences = &mut self.thread.process().references.lock();

        let handle = kunwrap!(Hid::from_raw(handle));

        let res = match refs.get(handle).cloned() {
            Some(h) => {
                let new = refs.insert(h);
                cloned.write(new.0.get());
                SyscallResult::Ok
            }
            None => SyscallResult::UnknownHandle,
        };
        ControlFlow::Continue(res.into_raw())
    }

    fn raw_sys_object_type(
        &mut self,
        handle: hid_t,
        ty: *mut usize,
    ) -> core::ops::ControlFlow<KernelSyscallHandlerBreak, result_t> {
        let bounds = get_current_bounds(self.thread.process());
        let mut ty = unsafe { kunwrap!(UserPtrMut::new(ty, bounds)) };

        let refs: &mut ProcessReferences = &mut self.thread.process().references.lock();

        let handle = kunwrap!(Hid::from_raw(handle));

        let res = match refs.get(handle) {
            Some(h) => {
                ty.write(h.object_type() as usize);
                SyscallResult::Ok
            }
            None => SyscallResult::UnknownHandle,
        };
        ControlFlow::Continue(res.into_raw())
    }

    fn raw_sys_object_wait(
        &mut self,
        handle: hid_t,
        on: signals_t,
        result: *mut signals_t,
    ) -> core::ops::ControlFlow<KernelSyscallHandlerBreak, result_t> {
        let bounds = get_current_bounds(self.thread.process());
        let mut result = unsafe { kunwrap!(UserPtrMut::new(result, bounds)) };

        let refs = self.thread.process().references.lock();

        let handle = kunwrap!(Hid::from_raw(handle));

        let Some(val) = refs.get(handle).cloned() else {
            return ControlFlow::Continue(SyscallResult::UnknownHandle.into_raw());
        };

        let mask = ObjectSignal::from_bits_truncate(on);

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
        ControlFlow::Continue(SyscallResult::Ok.into_raw())
    }

    fn raw_sys_object_wait_port(
        &mut self,
        handle: hid_t,
        port: hid_t,
        mask: signals_t,
        key: u64,
    ) -> core::ops::ControlFlow<KernelSyscallHandlerBreak, result_t> {
        let refs: &mut ProcessReferences = &mut self.thread.process().references.lock();

        let handle = kunwrap!(Hid::from_raw(handle));
        let port = kunwrap!(Hid::from_raw(port));

        let Some((handle, port)) = refs.get(handle).zip(refs.get(port)) else {
            return ControlFlow::Continue(SyscallResult::UnknownHandle.into_raw());
        };

        let mask = ObjectSignal::from_bits_truncate(mask);

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

        ControlFlow::Continue(SyscallResult::Ok.into_raw())
    }

    fn raw_sys_channel_create(
        &mut self,
        left: *mut hid_t,
        right: *mut hid_t,
    ) -> core::ops::ControlFlow<KernelSyscallHandlerBreak> {
        let bounds = get_current_bounds(self.thread.process());
        let mut left = kunwrap!(unsafe { UserPtrMut::new(left, bounds) });
        let mut right = kunwrap!(unsafe { UserPtrMut::new(right, bounds) });

        let (l, r) = channel_create();

        let l = self.thread.process().add_value(l.into());
        let r = self.thread.process().add_value(r.into());

        left.write(l.into_raw());
        right.write(r.into_raw());
        ControlFlow::Continue(())
    }

    fn raw_sys_channel_read(
        &mut self,
        handle: hid_t,
        data: *mut u8,
        data_len: *mut usize,
        handles: *mut hid_t,
        handles_len: *mut usize,
    ) -> core::ops::ControlFlow<KernelSyscallHandlerBreak, result_t> {
        let bounds = get_current_bounds(self.thread.process());

        let mut data_len = unsafe { kunwrap!(UserPtrMut::new(data_len, bounds)) };
        let mut handles_len = unsafe { kunwrap!(UserPtrMut::new(handles_len, bounds)) };

        let handle = kunwrap!(Hid::from_raw(handle));
        let handle = kunwrap!(self.thread.process().get_value(handle));
        let chan = kenum_cast!(handle, KernelValue::Channel);

        let res = match chan.read(data_len.read(), handles_len.read()) {
            Ok(ok) => {
                data_len.write(ok.data.len());
                let mut data_buf =
                    unsafe { kunwrap!(UserBytesMut::new(data, ok.data.len(), bounds)) };

                data_buf.write(&ok.data);

                if let Some(h) = ok.handles {
                    handles_len.write(h.len());

                    let mut handle_ptr = handles;
                    for handle in h {
                        let id = self.thread.process().add_value(handle);
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
        };
        ControlFlow::Continue(res.into_raw())
    }

    fn raw_sys_channel_write(
        &mut self,
        handle: hid_t,
        data: *const u8,
        data_len: usize,
        handles: *const hid_t,
        handles_len: usize,
    ) -> core::ops::ControlFlow<KernelSyscallHandlerBreak, result_t> {
        let bounds = get_current_bounds(self.thread.process());
        let data = unsafe { kunwrap!(UserBytes::new(data, data_len, bounds)) };

        let handle = kunwrap!(Hid::from_raw(handle));
        let handle = kunwrap!(self.thread.process().get_value(handle));
        let chan = kenum_cast!(handle, KernelValue::Channel);

        let handles = if !handles.is_null() && handles_len > 0 {
            let mut handles_res = Vec::with_capacity(handles_len);
            let refs = self.thread.process().references.lock();
            for i in 0..handles_len {
                let h = unsafe { kunwrap!(UserPtr::new(handles.wrapping_add(i), bounds)) };
                let r = kunwrap!(Hid::from_raw(h.read()));
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
        ControlFlow::Continue(chan.send(msg).into_raw())
    }

    fn raw_sys_interrupt_create(
        &mut self,
    ) -> core::ops::ControlFlow<KernelSyscallHandlerBreak, hid_t> {
        let interrupt = KInterruptHandle::new();
        let id = self.thread.process().add_value(Arc::new(interrupt).into());
        ControlFlow::Continue(id.0.get())
    }

    fn raw_sys_interrupt_wait(
        &mut self,
        handle: hid_t,
    ) -> core::ops::ControlFlow<KernelSyscallHandlerBreak, result_t> {
        let handle = kunwrap!(Hid::from_raw(handle));
        let int = kunwrap!(self.thread.process().get_value(handle));
        let int = kenum_cast!(int, KernelValue::Interrupt);
        ControlFlow::Continue(int.wait().into_raw())
    }

    fn raw_sys_interrupt_trigger(
        &mut self,
        handle: hid_t,
    ) -> core::ops::ControlFlow<KernelSyscallHandlerBreak, result_t> {
        let handle = kunwrap!(Hid::from_raw(handle));
        let int = kunwrap!(self.thread.process().get_value(handle));
        let int = kenum_cast!(int, KernelValue::Interrupt);
        int.trigger();
        ControlFlow::Continue(SyscallResult::Ok.into_raw())
    }

    fn raw_sys_interrupt_acknowledge(
        &mut self,
        handle: hid_t,
    ) -> core::ops::ControlFlow<KernelSyscallHandlerBreak, result_t> {
        let handle = kunwrap!(Hid::from_raw(handle));
        let int = kunwrap!(self.thread.process().get_value(handle));
        let int = kenum_cast!(int, KernelValue::Interrupt);
        int.ack();
        ControlFlow::Continue(SyscallResult::Ok.into_raw())
    }

    fn raw_sys_interrupt_set_port(
        &mut self,
        handle: hid_t,
        port: hid_t,
        key: u64,
    ) -> core::ops::ControlFlow<KernelSyscallHandlerBreak, result_t> {
        let int = kunwrap!(Hid::from_raw(handle));
        let int = kunwrap!(self.thread.process().get_value(int));
        let int = kenum_cast!(int, KernelValue::Interrupt);

        let port = kunwrap!(Hid::from_raw(port));
        let port = kunwrap!(self.thread.process().get_value(port));
        let port = kenum_cast!(port, KernelValue::Port);
        int.set_port(port, key);
        ControlFlow::Continue(SyscallResult::Ok.into_raw())
    }

    fn raw_sys_port_create(&mut self) -> core::ops::ControlFlow<KernelSyscallHandlerBreak, hid_t> {
        let interrupt = KPort::new();
        let id = self.thread.process().add_value(Arc::new(interrupt).into());
        ControlFlow::Continue(id.0.get())
    }

    fn raw_sys_port_wait(
        &mut self,
        handle: hid_t,
        result: *mut sys_port_notification_t,
    ) -> core::ops::ControlFlow<KernelSyscallHandlerBreak, result_t> {
        let bounds = get_current_bounds(self.thread.process());
        let mut result = unsafe { kunwrap!(UserPtrMut::new(result, bounds)) };

        let handle = kunwrap!(Hid::from_raw(handle));
        let handle = kunwrap!(self.thread.process().get_value(handle));
        let port = kenum_cast!(handle, KernelValue::Port);

        result.write(port.wait().into_raw());

        ControlFlow::Continue(SyscallResult::Ok.into_raw())
    }

    fn raw_sys_port_push(
        &mut self,
        handle: hid_t,
        notification: *const sys_port_notification_t,
    ) -> core::ops::ControlFlow<KernelSyscallHandlerBreak, result_t> {
        let bounds = get_current_bounds(self.thread.process());
        let value = unsafe { kunwrap!(UserPtr::new(notification, bounds)) };

        let handle = kunwrap!(Hid::from_raw(handle));
        let handle = kunwrap!(self.thread.process().get_value(handle));
        let port = kenum_cast!(handle, KernelValue::Port);

        let value = kunwrap!(SysPortNotification::from_raw(value.read()));

        port.notify(value);

        ControlFlow::Continue(SyscallResult::Ok.into_raw())
    }

    fn raw_sys_process_spawn_thread(
        &mut self,
        func: *const (),
        arg: *mut (),
    ) -> core::ops::ControlFlow<KernelSyscallHandlerBreak, tid_t> {
        let tid = unsafe { taskmanager::spawn_thread(func as usize, arg as usize).into_raw() };
        ControlFlow::Continue(tid)
    }

    fn raw_sys_process_exit_code(
        &mut self,
        handle: hid_t,
        exit: *mut usize,
    ) -> core::ops::ControlFlow<KernelSyscallHandlerBreak, result_t> {
        let bounds = get_current_bounds(self.thread.process());
        let mut exit = unsafe { kunwrap!(UserPtrMut::new(exit, bounds)) };

        let handle = kunwrap!(Hid::from_raw(handle));
        let handle = kunwrap!(self.thread.process().get_value(handle));
        let proc = kenum_cast!(handle, KernelValue::Process);
        let status = *proc.exit_status.lock();
        let res = match status {
            Some(val) => {
                exit.write(val);
                SyscallResult::Ok
            }
            None => SyscallResult::ProcessStillRunning,
        };
        ControlFlow::Continue(res.into_raw())
    }

    fn raw_sys_message_create(
        &mut self,
        data: *const u8,
        data_len: usize,
    ) -> core::ops::ControlFlow<KernelSyscallHandlerBreak, hid_t> {
        let bounds = get_current_bounds(self.thread.process());
        let data = unsafe { kunwrap!(UserBytes::new(data, data_len, bounds)) };

        let msg = Arc::new(KMessage {
            data: data.read_to_box(),
        });

        ControlFlow::Continue(self.thread.process().add_value(msg.into()).into_raw())
    }

    fn raw_sys_message_size(
        &mut self,
        handle: hid_t,
        size: *mut usize,
    ) -> core::ops::ControlFlow<KernelSyscallHandlerBreak, result_t> {
        let bounds = get_current_bounds(self.thread.process());
        let mut size = unsafe { kunwrap!(UserPtrMut::new(size, bounds)) };

        let handle = kunwrap!(Hid::from_raw(handle));
        let handle = kunwrap!(self.thread.process().get_value(handle));
        let message = kenum_cast!(handle, KernelValue::Message);
        size.write(message.data.len());
        ControlFlow::Continue(SyscallResult::Ok.into_raw())
    }

    fn raw_sys_message_read(
        &mut self,
        handle: hid_t,
        buf: *mut u8,
        buf_len: usize,
    ) -> core::ops::ControlFlow<KernelSyscallHandlerBreak, result_t> {
        let bounds = get_current_bounds(self.thread.process());
        let mut buffer = unsafe { kunwrap!(UserBytesMut::new(buf, buf_len, bounds)) };

        let handle = kunwrap!(Hid::from_raw(handle));
        let handle = kunwrap!(self.thread.process().get_value(handle));
        let message = kenum_cast!(handle, KernelValue::Message);

        kassert!(
            message.data.len() == buf_len,
            "Data and loc len should be same instead was: {} {}",
            message.data.len(),
            buf_len
        );

        buffer.write(&message.data);

        ControlFlow::Continue(SyscallResult::Ok.into_raw())
    }

    fn raw_sys_vmo_mmap_create(
        &mut self,
        base: *mut (),
        length: usize,
    ) -> core::ops::ControlFlow<KernelSyscallHandlerBreak, hid_t> {
        kassert!(
            self.thread.process().privilege == ProcessPrivilege::KERNEL,
            "MMAP is privileged"
        );
        let hid = unsafe {
            let vmo = Arc::new(Spinlock::new(VMO::new_mmap(base as usize, length)));
            self.thread.process().references.lock().insert(vmo.into())
        };
        ControlFlow::Continue(hid.into_raw())
    }

    fn raw_sys_vmo_anonymous_create(
        &mut self,
        length: usize,
        flags: u32,
    ) -> core::ops::ControlFlow<KernelSyscallHandlerBreak, hid_t> {
        let flags = VMOAnonymousFlags::from_bits_truncate(flags);

        if flags.intersects(VMOAnonymousFlags::_PRIVILEGED)
            && self.thread.process().privilege == ProcessPrivilege::USER
        {
            kpanic!("Only kernel can use privileged flags")
        }

        let vmo = Arc::new(Spinlock::new(VMO::new_anonymous(length, flags)));
        let hid = self.thread.process().references.lock().insert(vmo.into());
        ControlFlow::Continue(hid.into_raw())
    }

    fn raw_sys_vmo_anonymous_pinned_addresses(
        &mut self,
        handle: hid_t,
        offset: usize,
        length: usize,
        result: *mut usize,
    ) -> core::ops::ControlFlow<KernelSyscallHandlerBreak, result_t> {
        kassert!(self.thread.process().privilege == ProcessPrivilege::KERNEL);

        let handle = kunwrap!(Hid::from_raw(handle));

        let refs = self.thread.process().references.lock();
        let val = kunwrap!(refs.get(handle));
        let vmo = kenum_cast!(val, KernelValue::VMO);
        match &*vmo.lock() {
            VMO::MemoryMapped { .. } => kpanic!("not anonymous"),
            VMO::Anonymous { flags, pages } => {
                kassert!(flags.contains(VMOAnonymousFlags::PINNED));
                let bounds = get_current_bounds(self.thread.process());

                for (i, p) in pages.iter().skip(offset).enumerate().take(length) {
                    let mut ptr = unsafe { kunwrap!(UserPtrMut::new(result.add(i), bounds)) };
                    ptr.write(p.map(|v| v.get_address() as usize).unwrap_or(0));
                }
            }
        }
        ControlFlow::Continue(SyscallResult::Ok.into_raw())
    }
}
