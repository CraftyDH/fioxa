use alloc::{
    boxed::Box,
    string::String,
    vec::{self, Vec},
};

use crate::{
    ids::{ProcessID, ServiceID},
    proc::{PID, TID},
    service::{SendError, ServiceMessage, ServiceMessageContainer, ServiceTrackingNumber},
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
pub const SERVICE_GET: usize = 4;

pub const READ_ARGS: usize = 8;

pub const GET_PID: usize = 9;

lazy_static::lazy_static! {
    // ! BEWARE, DO NOT USE THIS FROM THE KERNEL
    // As it is static is won't give the correct answer
    pub static ref CURRENT_PID: ProcessID = get_pid();
}

unsafe fn syscall(syscall: usize) -> usize {
    let result: usize;
    core::arch::asm!("int 0x80", in("rax") syscall, lateout("rax") result, options(nostack));
    result
}

unsafe fn syscall1(syscall: usize, arg: usize) -> usize {
    let result: usize;
    core::arch::asm!("int 0x80", in("rax") syscall, in("r8") arg, lateout("rax") result, options(nostack));
    result
}

unsafe fn syscall2(syscall: usize, arg1: usize, arg2: usize) -> usize {
    let result: usize;
    core::arch::asm!("int 0x80", in("rax") syscall, in("r8") arg1, in("r9") arg2, lateout("rax") result, options(nostack));
    result
}

unsafe fn syscall3(syscall: usize, arg1: usize, arg2: usize, arg3: usize) -> usize {
    let result: usize;
    core::arch::asm!("int 0x80", in("rax") syscall, in("r8") arg1, in("r9") arg2, in("r10") arg3, lateout("rax") result, options(nostack));
    result
}

unsafe fn syscall3_2(syscall: usize, arg1: usize, arg2: usize, arg3: usize) -> (usize, usize) {
    let result: usize;
    let result2: usize;
    core::arch::asm!("int 0x80", in("rax") syscall, in("r8") arg1, in("r9") arg2, in("r10") arg3, lateout("rax") result, lateout("r8") result2, options(nostack));
    (result, result2)
}

unsafe fn syscall4(syscall: usize, arg1: usize, arg2: usize, arg3: usize, arg4: usize) -> usize {
    let result: usize;
    core::arch::asm!("int 0x80", in("rax") syscall, in("r8") arg1, in("r9") arg2, in("r10") arg3, in("r11") arg4, lateout("rax") result, options(nostack));
    result
}

#[inline]
pub fn echo(num: usize) -> usize {
    unsafe { syscall1(ECHO, num) }
}

#[inline]
pub fn yield_now() {
    unsafe { syscall(YIELD_NOW) };
}

pub fn spawn_process<F>(func: F, args: &str, kernel: bool) -> PID
where
    F: Fn() + Send + Sync,
{
    let boxed_func: Box<dyn Fn()> = Box::new(func);
    let raw = Box::into_raw(Box::new(boxed_func)) as *mut usize;

    let privilege = if kernel { 1 } else { 0 };

    let res = unsafe {
        syscall4(
            SPAWN_PROCESS,
            raw as usize,
            args.as_ptr() as usize,
            args.len(),
            privilege,
        )
    } as u64;
    PID::from(res)
}

pub fn spawn_thread<F>(func: F) -> TID
where
    F: FnOnce() + Send + Sync,
{
    let boxed_func: Box<dyn FnOnce()> = Box::new(func);
    let raw = Box::into_raw(Box::new(boxed_func)) as *mut usize;
    let res = unsafe { syscall1(SPAWN_THREAD, raw as usize) } as u64;
    res.into()
}

#[inline]
pub fn mmap_page(vmem: usize) {
    unsafe { syscall1(MMAP_PAGE, vmem) };
}

pub fn send_service_message(msg: &ServiceMessage) -> Result<(), SendError> {
    let data = postcard::to_allocvec(msg).unwrap();

    let error = unsafe { syscall3(SERVICE, SERVICE_PUSH, data.as_ptr() as usize, data.len()) };

    SendError::try_decode(error)?;

    Ok(())
}

pub fn poll_receive_service_message(id: ServiceID) -> Option<ServiceMessageContainer> {
    poll_receive_service_message_tracking(id, ServiceTrackingNumber(u64::MAX))
}

pub fn poll_receive_service_message_tracking(
    id: ServiceID,
    tracking_number: ServiceTrackingNumber,
) -> Option<ServiceMessageContainer> {
    unsafe {
        let length = syscall3(
            SERVICE,
            SERVICE_FETCH,
            id.0 as usize,
            tracking_number.0 as usize,
        );

        if length == 0 {
            return None;
        }

        let mut buf: Vec<u8> = Vec::with_capacity(length);

        let result = syscall3(SERVICE, SERVICE_GET, buf.as_mut_ptr() as usize, length);

        if result != 0 {
            panic!("Error getting message")
        }

        buf.set_len(length);

        Some(ServiceMessageContainer { buffer: buf })
    }
}

pub fn wait_receive_service_message(id: ServiceID) -> ServiceMessageContainer {
    wait_receive_service_message_tracking(id, ServiceTrackingNumber(u64::MAX))
}

pub fn wait_receive_service_message_tracking(
    id: ServiceID,
    tracking_number: ServiceTrackingNumber,
) -> ServiceMessageContainer {
    loop {
        if let Some(msg) = poll_receive_service_message_tracking(id, tracking_number) {
            return msg;
        }
        yield_now()
    }
}

pub fn send_and_wait_response_service_message(
    msg: &ServiceMessage,
) -> Result<ServiceMessageContainer, SendError> {
    let id = msg.service_id;
    let tracking = msg.tracking_number;
    send_service_message(msg)?;
    Ok(wait_receive_service_message_tracking(id, tracking))
}

pub fn service_create() -> ServiceID {
    unsafe {
        let sid = syscall1(SERVICE, SERVICE_CREATE);
        ServiceID(sid.try_into().unwrap())
    }
}

pub fn service_subscribe(id: ServiceID) {
    unsafe {
        syscall2(SERVICE, SERVICE_SUBSCRIBE, id.0 as usize);
    }
}

pub fn read_args() -> String {
    unsafe {
        let size = syscall1(READ_ARGS, 0);

        let buf: vec::Vec<u8> = vec![0u8; size];

        syscall1(READ_ARGS, buf.as_ptr() as usize);

        String::from_utf8(buf).unwrap()
    }
}

pub fn read_args_raw() -> vec::Vec<u8> {
    unsafe {
        let size = syscall1(READ_ARGS, 0);

        let buf: vec::Vec<u8> = vec![0u8; size];

        syscall1(READ_ARGS, buf.as_ptr() as usize);

        buf
    }
}

pub fn exit() -> ! {
    unsafe {
        syscall(EXIT_THREAD);

        loop {
            core::arch::asm!("hlt")
        }
    }
}

pub fn get_pid() -> ProcessID {
    unsafe {
        let pid = syscall(GET_PID);
        ProcessID(pid as u64)
    }
}
