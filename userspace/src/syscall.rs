use alloc::{boxed::Box, string::String, vec};
use kernel_userspace::{stream::StreamMessage, syscall};

use crate::proc::TID;

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
    unsafe { syscall1(syscall::ECHO, num) }
}

#[inline]
pub fn yield_now() {
    unsafe { syscall(syscall::YIELD_NOW) };
}

pub fn spawn_thread<F>(func: F) -> TID
where
    F: FnOnce() + Send + Sync,
{
    let boxed_func: Box<dyn FnOnce()> = Box::new(func);
    let raw = Box::into_raw(Box::new(boxed_func)) as *mut usize;
    let res = unsafe { syscall1(syscall::SPAWN_THREAD, raw as usize) } as u64;
    res.into()
}

#[inline]
pub fn mmap_page(vmem: usize) {
    unsafe { syscall1(syscall::MMAP_PAGE, vmem) };
}

pub fn stream_conn(name: &str) -> Option<usize> {
    unsafe {
        let (success, number) = syscall3_2(
            syscall::STREAM,
            syscall::STREAM_PUSH,
            name.as_ptr() as usize,
            name.len(),
        );
        if success == 0 {
            Some(number)
        } else {
            None
        }
    }
}

pub fn stream_pop(number: usize) -> Option<StreamMessage> {
    let mut v = unsafe { core::mem::zeroed() };
    unsafe {
        let x = syscall3(
            syscall::STREAM,
            syscall::STREAM_POP,
            number,
            &mut v as *mut StreamMessage as usize,
        );
        if x == 0 {
            Some(v)
        } else {
            None
        }
    }
}

pub fn stream_push(number: usize, value: StreamMessage) {
    let mut rax: usize = 1;
    while rax != 0 {
        unsafe {
            rax = syscall3(
                syscall::STREAM,
                syscall::STREAM_PUSH,
                number,
                &value as *const StreamMessage as usize,
            )
        };
        if rax == 1 {
            yield_now()
        }
    }
}

pub fn read_args() -> String {
    unsafe {
        let size = syscall1(syscall::READ_ARGS, 0);

        let buf = vec![0u8; size];

        syscall1(syscall::READ_ARGS, buf.as_ptr() as usize);

        String::from_utf8(buf).unwrap()
    }
}

pub fn exit() -> ! {
    unsafe {
        syscall(syscall::EXIT_THREAD);

        loop {
            core::arch::asm!("hlt")
        }
    }
}
