use super::types::*;

pub const SYSCALL_NUMBER: u8 = 0x80;

macro_rules! define_syscalls {
    ($($t:tt)*) => {
        #[cfg(target_abi = "kernel")]
        kernel_syscall::define_syscalls! {"kernel", $($t)*}
        #[cfg(target_abi = "driver")]
        kernel_syscall::define_syscalls! {"driver", $($t)*}
        #[cfg(target_abi = "userspace")]
        kernel_syscall::define_syscalls! {"userspace", $($t)*}
        #[cfg(not(any(target_abi = "kernel", target_abi = "driver", target_abi = "userspace")))]
        kernel_syscall::define_syscalls! {"none", $($t)*}
    };
}

define_syscalls! {
    // misc
    RawSysEcho @ raw_sys_echo(val: usize, val2: *mut usize),
    RawSysYield @ raw_sys_yield(),
    RawSysSleep @ raw_sys_sleep(ms: u64, slept: *mut u64),
    RawSysUptime @ raw_sys_uptime(time: *mut u64),
    RawSysExit @ raw_sys_exit(),
    RawSysMap @ raw_sys_map(vmo: /*optional*/ hid_t, flags: u32, hint: vaddr_t, length: usize, result: *mut vaddr_t),
    RawSysUnmap @ raw_sys_unmap(address: vaddr_t, length: usize),
    RawSysReadArgs @ raw_sys_read_args(buffer: *mut u8, len: usize, out_len: *mut usize),
    RawSysPid @ raw_sys_pid(pid: *mut pid_t),
    RawSysLog @ raw_sys_log(level: u32, target: *const u8, target_len: usize, message: *const u8, message_len: usize),

    // handle
    RawSysHandleDrop @ raw_sys_handle_drop(handle: hid_t),
    RawSysHandleClone @ raw_sys_handle_clone(handle: hid_t, cloned: *mut hid_t),

    // object
    RawSysObjectType @ raw_sys_object_type(handle: hid_t, ty: *mut usize),
    RawSysObjectWait @ raw_sys_object_wait(handle: hid_t, on: signals_t, result: *mut signals_t),
    RawSysObjectWaitPort @ raw_sys_object_wait_port(handle: hid_t, port: hid_t, mask: signals_t, key: u64),

    // channel
    RawSysChannelCreate @ raw_sys_channel_create(left: *mut hid_t, right: *mut hid_t),
    RawSysChannelRead @ raw_sys_channel_read(handle: hid_t, data: *mut u8, data_len: *mut usize, handles: *mut hid_t, handles_len: *mut usize),
    RawSysChannelWrite @ raw_sys_channel_write(handle: hid_t, data: *const u8, data_len: usize, handles: *const hid_t, handles_len: usize),

    // interrupt
    RawSysInterruptCreate @ raw_sys_interrupt_create(out: *mut hid_t),
    RawSysInterruptWait @ raw_sys_interrupt_wait(handle: hid_t),
    RawSysInterruptTrigger @ raw_sys_interrupt_trigger(handle: hid_t),
    RawSysInterruptAcknowledge @ raw_sys_interrupt_acknowledge(handle: hid_t),
    RawSysInterruptSetPort @ raw_sys_interrupt_set_port(handle: hid_t, port: hid_t, key: u64),

    // port
    RawSysPortCreate @ raw_sys_port_create(out: *mut hid_t),
    RawSysPortWait @ raw_sys_port_wait(handle: hid_t, result: *mut sys_port_notification_t),
    RawSysPortPush @ raw_sys_port_push(handle: hid_t, notification: *const sys_port_notification_t),

    // process
    RawSysProcessSpawnThread @ raw_sys_process_spawn_thread(func: *const (), arg: *mut (), out: *mut tid_t),
    RawSysProcessExitCode @ raw_sys_process_exit_code(handle: hid_t, exit: *mut usize),

    // message
    RawSysMessageCreate @ raw_sys_message_create(data: *const u8, data_len: usize, out: *mut hid_t),
    RawSysMessageSize @ raw_sys_message_size(handle: hid_t, size: *mut usize),
    RawSysMessageRead @ raw_sys_message_read(handle: hid_t, buf: *mut u8, buf_len: usize),

    // vmo
    RawSysVMOMMAPCreate @ raw_sys_vmo_mmap_create(base: *mut (), length: usize, out: *mut hid_t),
    RawSysVMOAnonCreate @ raw_sys_vmo_anonymous_create(length: usize, flags: u32, out: *mut hid_t),
    RawSysVMOAnonPinned @ raw_sys_vmo_anonymous_pinned_addresses(handle: hid_t, offset: usize, length: usize, result: *mut usize),

    // futex
    RawSysFutexWait @ raw_sys_futex_wait(addr: *const usize, flags: u32, val: usize),
    RawSysFutexWake @ raw_sys_futex_wake(addr: *const usize, flags: u32, count: usize, woken: *mut usize),
}
