use core::mem::MaybeUninit;

use alloc::{boxed::Box, string::String, vec};
use conquer_once::spin::Lazy;

use crate::{
    ids::{ProcessID, ServiceID, ThreadID},
    message::MessageHandle,
    service::{ServiceMessage, ServiceMessageDesc, ServiceMessageK, ServiceTrackingNumber},
};

pub const SYSCALL_NUMBER: usize = 0x80;

// Syscall
// RAX: number
//

// Syscalls
pub const ECHO: usize = 0;
pub const YIELD_NOW: usize = 1;
pub const SPAWN_PROCESS: usize = 2;
pub const SPAWN_THREAD: usize = 3;
pub const SLEEP: usize = 4;
pub const EXIT_THREAD: usize = 5;
pub const MMAP_PAGE: usize = 6;

pub const SERVICE: usize = 7;

pub const SERVICE_CREATE: usize = 0;
pub const SERVICE_SUBSCRIBE: usize = 1;
pub const SERVICE_PUSH: usize = 2;
pub const SERVICE_FETCH: usize = 3;
pub const SERVICE_WAIT: usize = 4;

pub const READ_ARGS: usize = 8;

pub const GET_PID: usize = 9;
pub const UNMMAP_PAGE: usize = 10;
pub const MMAP_PAGE32: usize = 11;

pub const INTERNAL_KERNEL_WAKER: usize = 12;

pub const MESSAGE: usize = 13;

// ! BEWARE, DO NOT USE THIS FROM THE KERNEL
// As it is static is won't give the correct answer
pub static CURRENT_PID: Lazy<ProcessID> = Lazy::new(get_pid);

// TODO: Use fancier macros to dynamically build the argss
#[macro_export]
macro_rules! make_syscall {
    // No result
    ($syscall:expr) => {
        core::arch::asm!("int 0x80", in("rax") $syscall, options(nostack))
    };
    ($syscall:expr, $arg1:expr) => {
        core::arch::asm!("int 0x80", in("rax") $syscall, in("r8") $arg1, options(nostack))
    };
    ($syscall:expr, $arg1:expr, $arg2:expr) => {
        core::arch::asm!("int 0x80", in("rax") $syscall, in("r8") $arg1, in("r9") $arg2, options(nostack))
    };
    ($syscall:expr, $arg1:expr, $arg2:expr, $arg3:expr) => {
        core::arch::asm!("int 0x80", in("rax") $syscall, in("r8") $arg1, in("r9") $arg2, in("r10") $arg3, options(nostack))
    };

    // 1 result
    ($syscall:expr => $result:ident) => {
        core::arch::asm!("int 0x80", in("rax") $syscall, lateout("rax") $result, options(nostack))
    };
    ($syscall:expr, $arg1:expr => $result:ident) => {
        core::arch::asm!("int 0x80", in("rax") $syscall, in("r8") $arg1, lateout("rax") $result, options(nostack))
    };
    ($syscall:expr, $arg1:expr, $arg2:expr => $result:ident) => {
        core::arch::asm!("int 0x80", in("rax") $syscall, in("r8") $arg1, in("r9") $arg2, lateout("rax") $result, options(nostack))
    };
    ($syscall:expr, $arg1:expr, $arg2:expr, $arg3:expr => $result:ident) => {
        core::arch::asm!("int 0x80", in("rax") $syscall, in("r8") $arg1, in("r9") $arg2, in("r10") $arg3, lateout("rax") $result, options(nostack))
    };
    ($syscall:expr, $arg1:expr, $arg2:expr, $arg3:expr, $arg4:expr => $result:ident) => {
        core::arch::asm!("int 0x80", in("rax") $syscall, in("r8") $arg1, in("r9") $arg2, in("r10") $arg3, in("r11") $arg4, lateout("rax") $result, options(nostack))
    };

    // 2 results
    ($syscall:expr, $arg1:expr, $arg2:expr, $arg3:expr => $result:ident, $result2:ident) => {
        core::arch::asm!("int 0x80", in("rax") $syscall, in("r8") $arg1, in("r9") $arg2, in("r10") $arg3, lateout("rax") $result, lateout("r8") $result2, options(nostack))
    };
}

#[inline]
pub fn echo(num: usize) -> usize {
    let result;
    unsafe { make_syscall!(ECHO, num => result) }
    result
}

#[inline]
pub fn yield_now() {
    unsafe { make_syscall!(YIELD_NOW) };
}

pub fn spawn_process<F>(func: F, args: &[u8], kernel: bool) -> ProcessID
where
    F: Fn() + Send + Sync + 'static,
{
    let boxed_func: Box<dyn Fn()> = Box::new(func);
    let raw = Box::into_raw(Box::new(boxed_func)) as *mut usize;

    let privilege = if kernel { 1 } else { 0 };

    let res: u64;
    unsafe {
        make_syscall!(
            SPAWN_PROCESS,
            raw as usize,
            args.as_ptr() as usize,
            args.len(),
            privilege
            => res
        )
    }
    ProcessID(res)
}

pub fn spawn_thread<F>(func: F) -> ThreadID
where
    F: FnOnce() + Send + Sync + 'static,
{
    let boxed_func: Box<dyn FnOnce()> = Box::new(func);
    let raw = Box::into_raw(Box::new(boxed_func)) as *mut usize;
    let res: u64;
    unsafe { make_syscall!(SPAWN_THREAD, thread_bootstraper, raw => res) }
    ThreadID(res)
}

/// Is used as a new threads entry point.
pub unsafe extern "C" fn thread_bootstraper(main: usize) {
    // Recreate the function box that was passed from the syscall
    let func = unsafe { Box::from_raw(main as *mut Box<dyn FnOnce()>) };
    // We can release the outer box
    let func = Box::into_inner(func);
    // Call the function
    func.call_once(());

    // Function ended quit
    exit()
}

#[inline]
pub fn mmap_page(vmem: usize, length: usize) -> usize {
    let mem;
    unsafe { make_syscall!(MMAP_PAGE, vmem, length => mem) };
    mem
}

#[inline]
pub fn mmap_page32() -> u32 {
    let res: u32;
    unsafe { make_syscall!(MMAP_PAGE32 => res) };
    res
}

#[inline]
pub fn unmmap_page(vmem: usize, mapping_length: usize) {
    unsafe { make_syscall!(UNMMAP_PAGE, vmem, mapping_length) };
}

pub fn send_service_message(msg: &ServiceMessageDesc, descriptor: &MessageHandle) {
    let msg = ServiceMessageK {
        service_id: msg.service_id,
        sender_pid: msg.sender_pid,
        tracking_number: msg.tracking_number,
        destination: msg.destination,
        descriptor: descriptor.id(),
    };

    unsafe { make_syscall!(SERVICE, SERVICE_PUSH, &msg) };
}

pub fn try_receive_service_message_tracking(
    id: ServiceID,
    tracking_number: ServiceTrackingNumber,
) -> Option<ServiceMessage>
where
{
    fetch_service_message(id, tracking_number)
}

pub fn try_receive_service_message(id: ServiceID) -> Option<ServiceMessage> {
    try_receive_service_message_tracking(id, ServiceTrackingNumber(u64::MAX))
}

pub fn receive_service_message_blocking_tracking(
    id: ServiceID,
    tracking_number: ServiceTrackingNumber,
) -> ServiceMessage {
    fetch_service_message_blocking(id, tracking_number)
}

pub fn receive_service_message_blocking(id: ServiceID) -> ServiceMessage {
    receive_service_message_blocking_tracking(id, ServiceTrackingNumber(u64::MAX))
}

pub fn fetch_service_message(
    id: ServiceID,
    tracking_number: ServiceTrackingNumber,
) -> Option<ServiceMessage> {
    unsafe {
        let res: usize;
        let mut msg = MaybeUninit::uninit();
        make_syscall!(
            SERVICE,
            SERVICE_FETCH,
            msg.as_mut_ptr(),
            id.0 as usize,
            tracking_number.0 as usize
            => res
        );

        if res == 0 {
            None
        } else {
            Some(msg.assume_init())
        }
    }
}

pub fn service_wait(id: ServiceID) {
    unsafe { make_syscall!(SERVICE, SERVICE_WAIT, id.0) }
}

pub fn fetch_service_message_blocking(
    id: ServiceID,
    tracking_number: ServiceTrackingNumber,
) -> ServiceMessage {
    loop {
        if let Some(r) = fetch_service_message(id, tracking_number) {
            return r;
        }
        service_wait(id);
    }
}

pub fn send_and_get_response_service_message(
    msg: &ServiceMessageDesc,
    descriptor: &MessageHandle,
) -> ServiceMessage {
    let id = msg.service_id;
    let tracking = msg.tracking_number;
    send_service_message(msg, descriptor);
    receive_service_message_blocking_tracking(id, tracking)
}

pub fn service_create() -> ServiceID {
    unsafe {
        let sid: usize;
        make_syscall!(SERVICE, SERVICE_CREATE => sid);
        ServiceID(sid.try_into().unwrap())
    }
}

pub fn service_subscribe(id: ServiceID) {
    unsafe {
        make_syscall!(SERVICE, SERVICE_SUBSCRIBE, id.0 as usize);
    }
}

pub fn read_args() -> String {
    unsafe {
        let size;
        make_syscall!(READ_ARGS, 0 => size);

        let buf: vec::Vec<u8> = vec![0u8; size];

        make_syscall!(READ_ARGS, buf.as_ptr() as usize);

        String::from_utf8(buf).unwrap()
    }
}

pub fn read_args_raw() -> vec::Vec<u8> {
    unsafe {
        let size;
        make_syscall!(READ_ARGS, 0 => size);

        let buf: vec::Vec<u8> = vec![0u8; size];

        make_syscall!(READ_ARGS, buf.as_ptr() as usize);

        buf
    }
}

pub fn exit() -> ! {
    unsafe {
        make_syscall!(EXIT_THREAD);

        loop {
            core::arch::asm!("hlt")
        }
    }
}

pub fn sleep(ms: u64) {
    unsafe { make_syscall!(SLEEP, ms) }
}

pub fn get_pid() -> ProcessID {
    unsafe {
        let pid: u64;
        make_syscall!(GET_PID => pid);
        ProcessID(pid)
    }
}

pub fn internal_kernel_waker_wait(id: usize) {
    unsafe {
        make_syscall!(INTERNAL_KERNEL_WAKER, id);
    }
}
