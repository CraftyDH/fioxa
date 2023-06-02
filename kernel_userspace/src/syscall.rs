use alloc::{boxed::Box, string::String, vec};

use crate::{
    proc::{PID, TID},
    stream::StreamMessage,
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
pub const STREAM: usize = 7;

pub const STREAM_PUSH: usize = 0;
pub const STREAM_POP: usize = 1;
pub const STREAM_GETID: usize = 2;

pub const STREAM_GETID_KB: usize = 1;
pub const STREAM_GETID_SOUT: usize = 2;

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

pub fn stream_pop() -> Option<StreamMessage> {
    let mut v = unsafe { core::mem::zeroed() };
    unsafe {
        let x = syscall2(STREAM, STREAM_POP, &mut v as *mut StreamMessage as usize);
        if x == 0 {
            Some(v)
        } else {
            None
        }
    }
}

pub fn stream_push(value: StreamMessage) {
    let mut rax: usize = 1;
    while rax != 0 {
        unsafe { rax = syscall2(STREAM, STREAM_PUSH, &value as *const StreamMessage as usize) };
        if rax == 1 {
            yield_now()
        }
    }
}

pub fn stream_get_id(number: usize) -> usize {
    let rax;
    unsafe { rax = syscall2(STREAM, STREAM_GETID, number) };
    rax
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
