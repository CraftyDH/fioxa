use alloc::{
    boxed::Box,
    string::String,
    vec::{self, Vec},
};
use conquer_once::spin::Lazy;
use serde::{Deserialize, Serialize};

use crate::{
    ids::{ProcessID, ServiceID},
    proc::{PID, TID},
    service::{SendError, ServiceMessage, ServiceTrackingNumber},
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
pub const SERVICE_FETCH_WAIT: usize = 4;
pub const SERVICE_GET: usize = 5;

pub const READ_ARGS: usize = 8;

pub const GET_PID: usize = 9;
pub const UNMMAP_PAGE: usize = 10;
pub const MMAP_PAGE32: usize = 11;

// ! BEWARE, DO NOT USE THIS FROM THE KERNEL
// As it is static is won't give the correct answer
pub static CURRENT_PID: Lazy<ProcessID> = Lazy::new(get_pid);

// TODO: Use fancier macros to dynamically build the argss
#[macro_export]
macro_rules! syscall {
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
    unsafe { syscall!(ECHO, num => result) }
    result
}

#[inline]
pub fn yield_now() {
    unsafe { syscall!(YIELD_NOW) };
}

pub fn spawn_process<F>(func: F, args: &[u8], kernel: bool) -> PID
where
    F: Fn() + Send + Sync + 'static,
{
    let boxed_func: Box<dyn Fn()> = Box::new(func);
    let raw = Box::into_raw(Box::new(boxed_func)) as *mut usize;

    let privilege = if kernel { 1 } else { 0 };

    let res: u64;
    unsafe {
        syscall!(
            SPAWN_PROCESS,
            raw as usize,
            args.as_ptr() as usize,
            args.len(),
            privilege
            => res
        )
    }
    PID::from(res)
}

pub fn spawn_thread<F>(func: F) -> TID
where
    F: FnOnce() + Send + Sync + 'static,
{
    let boxed_func: Box<dyn FnOnce()> = Box::new(func);
    let raw = Box::into_raw(Box::new(boxed_func)) as *mut usize;
    let res: u64;
    unsafe { syscall!(SPAWN_THREAD, raw => res) }
    res.into()
}

#[inline]
pub fn mmap_page(vmem: usize) {
    unsafe { syscall!(MMAP_PAGE, vmem) };
}

#[inline]
pub fn mmap_page32() -> u32 {
    let res: u32;
    unsafe { syscall!(MMAP_PAGE32 => res) };
    res
}

#[inline]
pub fn unmmap_page(vmem: usize) {
    unsafe { syscall!(UNMMAP_PAGE, vmem) };
}

pub fn send_service_message<T: Serialize>(
    msg: &ServiceMessage<T>,
    buffer: &mut Vec<u8>,
) -> Result<(), SendError> {
    // Calulate how big to make the buffer
    let size =
        postcard::serialize_with_flavor(&msg, postcard::ser_flavors::Size::default()).unwrap();
    unsafe {
        buffer.reserve(size);
        buffer.set_len(size);
    }

    let data = postcard::to_slice(&msg, buffer).unwrap();

    let error: usize;
    unsafe { syscall!(SERVICE, SERVICE_PUSH, data.as_ptr() as usize, data.len() => error) };

    SendError::try_decode(error)
}

pub fn try_receive_service_message_tracking<'a, R: Deserialize<'a>>(
    id: ServiceID,
    tracking_number: ServiceTrackingNumber,
    buffer: &'a mut Vec<u8>,
) -> Option<Result<ServiceMessage<R>, postcard::Error>>
where
{
    let size = fetch_service_message(id, tracking_number)?;
    buffer.reserve(size);
    unsafe {
        buffer.set_len(size);
    }
    Some(get_service_message(buffer))
}

pub fn try_receive_service_message<'a, R>(
    id: ServiceID,
    buffer: &'a mut Vec<u8>,
) -> Option<Result<ServiceMessage<R>, postcard::Error>>
where
    R: Deserialize<'a>,
{
    try_receive_service_message_tracking(id, ServiceTrackingNumber(u64::MAX), buffer)
}

pub fn receive_service_message_blocking_tracking<'a, R>(
    id: ServiceID,
    tracking_number: ServiceTrackingNumber,
    buffer: &'a mut Vec<u8>,
) -> Result<ServiceMessage<R>, postcard::Error>
where
    R: Deserialize<'a>,
{
    let size = fetch_service_message_blocking(id, tracking_number);
    buffer.reserve(size);
    unsafe {
        buffer.set_len(size);
    }
    get_service_message(buffer)
}

pub fn receive_service_message_blocking<'a, R: Deserialize<'a>>(
    id: ServiceID,
    buffer: &'a mut Vec<u8>,
) -> Result<ServiceMessage<R>, postcard::Error> {
    receive_service_message_blocking_tracking(id, ServiceTrackingNumber(u64::MAX), buffer)
}

pub fn fetch_service_message(
    id: ServiceID,
    tracking_number: ServiceTrackingNumber,
) -> Option<usize> {
    unsafe {
        let length;

        syscall!(
            SERVICE,
            SERVICE_FETCH,
            id.0 as usize,
            tracking_number.0 as usize
            => length
        );

        if length == 0 {
            None
        } else {
            Some(length)
        }
    }
}

pub fn fetch_service_message_blocking(
    id: ServiceID,
    tracking_number: ServiceTrackingNumber,
) -> usize {
    unsafe {
        let length;
        syscall!(
            SERVICE,
            SERVICE_FETCH_WAIT,
            id.0 as usize,
            tracking_number.0 as usize
            => length
        );

        if length == 0 {
            unreachable!("KERNEL DID BAD");
        }
        length
    }
}

pub fn get_service_message<'a, T>(buf: &'a mut [u8]) -> Result<ServiceMessage<T>, postcard::Error>
where
    T: Deserialize<'a>,
{
    unsafe {
        let result: usize;
        syscall!(SERVICE, SERVICE_GET, buf.as_mut_ptr() as usize, buf.len() => result);

        if result != 0 {
            panic!("Error getting message, ensure that fetch was called first & buffer was increased appropriately");
        }

        postcard::from_bytes(buf)
    }
}

pub fn send_and_get_response_service_message<'a, T, R>(
    msg: &ServiceMessage<T>,
    buffer: &'a mut Vec<u8>,
) -> Result<ServiceMessage<R>, SendError>
where
    T: Serialize,
    R: Deserialize<'a>,
{
    let id = msg.service_id;
    let tracking = msg.tracking_number;
    send_service_message(msg, buffer)?;
    receive_service_message_blocking_tracking(id, tracking, buffer)
        .map_err(|_| SendError::FailedToDecodeResponse)
}

pub fn service_create() -> ServiceID {
    unsafe {
        let sid: usize;
        syscall!(SERVICE, SERVICE_CREATE => sid);
        ServiceID(sid.try_into().unwrap())
    }
}

pub fn service_subscribe(id: ServiceID) {
    unsafe {
        syscall!(SERVICE, SERVICE_SUBSCRIBE, id.0 as usize);
    }
}

pub fn read_args() -> String {
    unsafe {
        let size;
        syscall!(READ_ARGS, 0 => size);

        let buf: vec::Vec<u8> = vec![0u8; size];

        syscall!(READ_ARGS, buf.as_ptr() as usize);

        String::from_utf8(buf).unwrap()
    }
}

pub fn read_args_raw() -> vec::Vec<u8> {
    unsafe {
        let size;
        syscall!(READ_ARGS, 0 => size);

        let buf: vec::Vec<u8> = vec![0u8; size];

        syscall!(READ_ARGS, buf.as_ptr() as usize);

        buf
    }
}

pub fn exit() -> ! {
    unsafe {
        syscall!(EXIT_THREAD);

        loop {
            core::arch::asm!("hlt")
        }
    }
}

pub fn get_pid() -> ProcessID {
    unsafe {
        let pid: u64;
        syscall!(GET_PID => pid);
        ProcessID(pid)
    }
}
