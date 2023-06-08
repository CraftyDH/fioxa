use core::mem::MaybeUninit;

use alloc::{boxed::Box, string::String, vec};

use crate::{
    proc::{PID, TID},
    service::{ReceiveMessageHeader, SendMessageHeader, SID},
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
pub const SERVICE_POP: usize = 3;
pub const SERVICE_GETDATA: usize = 4;

pub const READ_ARGS: usize = 8;

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

pub fn spawn_process<F>(func: F, args: &str) -> PID
where
    F: Fn() + Send + Sync,
{
    let boxed_func: Box<dyn Fn()> = Box::new(func);
    let raw = Box::into_raw(Box::new(boxed_func)) as *mut usize;

    let res = unsafe {
        syscall3(
            SPAWN_PROCESS,
            raw as usize,
            args.as_ptr() as usize,
            args.len(),
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

pub fn service_create() -> SID {
    unsafe {
        let sid = syscall1(SERVICE, SERVICE_CREATE);
        SID(sid.try_into().unwrap())
    }
}

pub fn service_subscribe(id: SID) {
    unsafe {
        syscall2(SERVICE, SERVICE_SUBSCRIBE, id.0 as usize);
    }
}

pub fn poll_service(id: SID, tracking_number: u64) -> Option<ReceiveMessageHeader> {
    unsafe {
        let mut msg: MaybeUninit<ReceiveMessageHeader> = MaybeUninit::uninit();
        let result = syscall4(
            SERVICE,
            SERVICE_POP,
            msg.as_mut_ptr() as usize,
            id.0 as usize,
            tracking_number as usize,
        );
        if result == 0 {
            return Some(msg.assume_init());
        } else {
            return None;
        }
    }
}

pub fn service_receive_msg() -> Option<ReceiveMessageHeader> {
    poll_service(SID(u64::MAX), u64::MAX)
}

pub fn service_get_data(loc: &mut [u8]) -> Option<()> {
    unsafe {
        let result = syscall2(SERVICE, SERVICE_GETDATA, loc.as_ptr() as usize);
        if result == 0 {
            Some(())
        } else {
            None
        }
    }
}

pub fn service_push_msg(msg: SendMessageHeader) -> Option<()> {
    unsafe {
        let result = syscall2(
            SERVICE,
            SERVICE_PUSH,
            &msg as *const SendMessageHeader as usize,
        );
        if result == 0 {
            return Some(());
        } else {
            return None;
        }
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
