use super::types::*;

pub const SYSCALL_NUMBER: u8 = 0x80;

kernel_syscall::define_syscalls! {
    // misc
    raw_sys_echo(val: usize) -> usize,
    raw_sys_yield(),
    raw_sys_sleep(ms: u64) -> u64,
    raw_sys_exit() -> !,
    raw_sys_map(vmo: /*optional*/ hid_t, flags: u32, hint: vaddr_t, length: usize, result: *mut vaddr_t) -> result_t,
    raw_sys_unmap(address: vaddr_t, length: usize) -> result_t,
    raw_sys_read_args(buffer: *mut u8, len: usize) -> usize,
    raw_sys_pid() -> pid_t,
    raw_sys_log(level: u32, target: *const u8, target_len: usize, message: *const u8, message_len: usize),

    // handle
    raw_sys_handle_drop(handle: hid_t) -> result_t,
    raw_sys_handle_clone(handle: hid_t, cloned: *mut hid_t) -> result_t,

    // object
    raw_sys_object_type(handle: hid_t, ty: *mut usize) -> result_t,
    raw_sys_object_wait(handle: hid_t, on: signals_t, result: *mut signals_t) -> result_t,
    raw_sys_object_wait_port(handle: hid_t, port: hid_t, mask: signals_t, key: u64) -> result_t,

    // channel
    raw_sys_channel_create(left: *mut hid_t, right: *mut hid_t),
    raw_sys_channel_read(handle: hid_t, data: *mut u8, data_len: *mut usize, handles: *mut hid_t, handles_len: *mut usize) -> result_t,
    raw_sys_channel_write(handle: hid_t, data: *const u8, data_len: usize, handles: *const hid_t, handles_len: usize) -> result_t,

    // interrupt
    raw_sys_interrupt_create() -> hid_t,
    raw_sys_interrupt_wait(handle: hid_t) -> result_t,
    raw_sys_interrupt_trigger(handle: hid_t) -> result_t,
    raw_sys_interrupt_acknowledge(handle: hid_t) -> result_t,
    raw_sys_interrupt_set_port(handle: hid_t, port: hid_t, key: u64) -> result_t,

    // port
    raw_sys_port_create() -> hid_t,
    raw_sys_port_wait(handle: hid_t, result: *mut sys_port_notification_t) -> result_t,
    raw_sys_port_push(handle: hid_t, notification: *const sys_port_notification_t) -> result_t,

    // process
    raw_sys_process_spawn_thread(func: *const (), arg: *mut ()) -> tid_t,
    raw_sys_process_exit_code(handle: hid_t, exit: *mut usize) -> result_t,

    // message
    raw_sys_message_create(data: *const u8, data_len: usize) -> hid_t,
    raw_sys_message_size(handle: hid_t, size: *mut usize) -> result_t,
    raw_sys_message_read(handle: hid_t, buf: *mut u8, buf_len: usize) -> result_t,

    // vmo
    raw_sys_vmo_mmap_create(base: *mut (), length: usize) -> hid_t,
    raw_sys_vmo_anonymous_create(length: usize, flags: u32) -> hid_t,
    raw_sys_vmo_anonymous_pinned_addresses(handle: hid_t, offset: usize, length: usize, result: *mut usize) -> result_t,
}
