use alloc::{boxed::Box, string::String, vec};
use conquer_once::spin::Lazy;

use crate::ids::{ProcessID, ThreadID};

pub const SYSCALL_NUMBER: usize = 0x80;

// Syscall
// RAX: number
//

// Syscalls
pub const ECHO: usize = 0;
pub const YIELD_NOW: usize = 1;
pub const SPAWN_THREAD: usize = 3;
pub const SLEEP: usize = 4;
pub const EXIT_THREAD: usize = 5;
pub const MMAP_PAGE: usize = 6;
pub const READ_ARGS: usize = 7;
pub const GET_PID: usize = 8;
pub const UNMMAP_PAGE: usize = 9;
pub const MMAP_PAGE32: usize = 10;
pub const MESSAGE: usize = 11;
pub const PORT: usize = 12;
pub const INTERRUPT: usize = 13;
pub const CHANNEL: usize = 14;
pub const OBJECT: usize = 15;
pub const PROCESS: usize = 16;

// ! BEWARE, DO NOT USE THIS FROM THE KERNEL
// As it is static is won't give the correct answer
pub static CURRENT_PID: Lazy<ProcessID> = Lazy::new(get_pid);

#[cfg(all(feature = "kernel", feature = "iret"))]
const _: () = compile_error!("kernel and iret syscall types are incompatable");

#[cfg(feature = "kernel")]
use core::sync::atomic::{AtomicU64, Ordering};

#[cfg(feature = "kernel")]
static SYSCALL_FN: AtomicU64 = AtomicU64::new(0);

#[cfg(feature = "kernel")]
pub unsafe fn get_syscall_fn() -> u64 {
    SYSCALL_FN.load(Ordering::Acquire)
}

#[cfg(feature = "kernel")]
pub unsafe fn set_syscall_fn(f: u64) {
    SYSCALL_FN.store(f, Ordering::Release);
}

#[cfg(feature = "kernel")]
#[macro_export]
macro_rules! make_syscall {
    // No result
    ($syscall:expr) => {
        core::arch::asm!("call rax", in("rax") crate::syscall::get_syscall_fn(), in("rdi") $syscall, lateout("rax") _, lateout("r11") _, lateout("r10") _, lateout("r9") _, lateout("r8") _, lateout("rdi") _, lateout("rsi") _, lateout("rdx") _, lateout("rcx") _, options(nostack))
    };
    ($syscall:expr, $arg1:expr) => {
        core::arch::asm!("call rax", in("rax") crate::syscall::get_syscall_fn(), in("rdi") $syscall, in("rsi") $arg1, lateout("rax") _, lateout("r11") _, lateout("r10") _, lateout("r9") _, lateout("r8") _, lateout("rdi") _, lateout("rsi") _, lateout("rdx") _, lateout("rcx") _, options(nostack))
    };
    ($syscall:expr, $arg1:expr, $arg2:expr) => {
        core::arch::asm!("call rax", in("rax") crate::syscall::get_syscall_fn(), in("rdi") $syscall, in("rsi") $arg1, in("rdx") $arg2, lateout("rax") _, lateout("r11") _, lateout("r10") _, lateout("r9") _, lateout("r8") _, lateout("rdi") _, lateout("rsi") _, lateout("rdx") _, lateout("rcx") _, options(nostack))
    };
    ($syscall:expr, $arg1:expr, $arg2:expr, $arg3:expr) => {
        core::arch::asm!("call rax", in("rax") crate::syscall::get_syscall_fn(), in("rdi") $syscall, in("rsi") $arg1, in("rdx") $arg2, in("rcx") $arg3, lateout("rax") _, lateout("r11") _, lateout("r10") _, lateout("r9") _, lateout("r8") _, lateout("rdi") _, lateout("rsi") _, lateout("rdx") _, lateout("rcx") _, options(nostack))
    };

    // 1 result
    ($syscall:expr => $result:ident) => {
        core::arch::asm!("call rax", in("rax") crate::syscall::get_syscall_fn(), in("rdi") $syscall, lateout("rax") $result, lateout("r11") _, lateout("r10") _, lateout("r9") _, lateout("r8") _, lateout("rdi") _, lateout("rsi") _, lateout("rdx") _, lateout("rcx") _, options(nostack))
    };
    ($syscall:expr, $arg1:expr => $result:ident) => {
        core::arch::asm!("call rax", in("rax") crate::syscall::get_syscall_fn(), in("rdi") $syscall, in("rsi") $arg1, lateout("rax") $result, lateout("r11") _, lateout("r10") _, lateout("r9") _, lateout("r8") _, lateout("rdi") _, lateout("rsi") _, lateout("rdx") _, lateout("rcx") _, options(nostack))
    };
    ($syscall:expr, $arg1:expr, $arg2:expr => $result:ident) => {
        core::arch::asm!("call rax", in("rax") crate::syscall::get_syscall_fn(), in("rdi") $syscall, in("rsi") $arg1, in("rdx") $arg2, lateout("rax") $result, lateout("r11") _, lateout("r10") _, lateout("r9") _, lateout("r8") _, lateout("rdi") _, lateout("rsi") _, lateout("rdx") _, lateout("rcx") _, options(nostack))
    };
    ($syscall:expr, $arg1:expr, $arg2:expr, $arg3:expr => $result:ident) => {
        core::arch::asm!("call rax", in("rax") crate::syscall::get_syscall_fn(), in("rdi") $syscall, in("rsi") $arg1, in("rdx") $arg2, in("rcx") $arg3, lateout("rax") $result, lateout("r11") _, lateout("r10") _, lateout("r9") _, lateout("r8") _, lateout("rdi") _, lateout("rsi") _, lateout("rdx") _, lateout("rcx") _, options(nostack))
    };
    ($syscall:expr, $arg1:expr, $arg2:expr, $arg3:expr, $arg4:expr => $result:ident) => {
        core::arch::asm!("call rax", in("rax") crate::syscall::get_syscall_fn(), in("rdi") $syscall, in("rsi") $arg1, in("rdx") $arg2, in("rcx") $arg3, in("r8") $arg4, lateout("rax") $result, lateout("r11") _, lateout("r10") _, lateout("r9") _, lateout("r8") _, lateout("rdi") _, lateout("rsi") _, lateout("rdx") _, lateout("rcx") _, options(nostack))
    };
    ($syscall:expr, $arg1:expr, $arg2:expr, $arg3:expr, $arg4:expr, $arg5:expr => $result:ident) => {
        core::arch::asm!("call rax", in("rax") crate::syscall::get_syscall_fn(), in("rdi") $syscall, in("rsi") $arg1, in("rdx") $arg2, in("rcx") $arg3, in("r8") $arg4, in("r9") $arg5, lateout("rax") $result, lateout("r11") _, lateout("r10") _, lateout("r9") _, lateout("r8") _, lateout("rdi") _, lateout("rsi") _, lateout("rdx") _, lateout("rcx") _, options(nostack))
    };
}

// TODO: Use fancier macros to dynamically build the argss
#[cfg(feature = "iret")]
#[macro_export]
macro_rules! make_syscall {
    // No result
    ($syscall:expr) => {
        core::arch::asm!("int 0x80", in("rdi") $syscall, lateout("rax") _, lateout("r11") _, lateout("r10") _, lateout("r9") _, lateout("r8") _, lateout("rdi") _, lateout("rsi") _, lateout("rdx") _, lateout("rcx") _, options(nostack))
    };
    ($syscall:expr, $arg1:expr) => {
        core::arch::asm!("int 0x80", in("rdi") $syscall, in("rsi") $arg1, lateout("rax") _, lateout("r11") _, lateout("r10") _, lateout("r9") _, lateout("r8") _, lateout("rdi") _, lateout("rsi") _, lateout("rdx") _, lateout("rcx") _, options(nostack))
    };
    ($syscall:expr, $arg1:expr, $arg2:expr) => {
        core::arch::asm!("int 0x80", in("rdi") $syscall, in("rsi") $arg1, in("rdx") $arg2, lateout("rax") _, lateout("r11") _, lateout("r10") _, lateout("r9") _, lateout("r8") _, lateout("rdi") _, lateout("rsi") _, lateout("rdx") _, lateout("rcx") _, options(nostack))
    };
    ($syscall:expr, $arg1:expr, $arg2:expr, $arg3:expr) => {
        core::arch::asm!("int 0x80", in("rdi") $syscall, in("rsi") $arg1, in("rdx") $arg2, in("rcx") $arg3, lateout("rax") _, lateout("r11") _, lateout("r10") _, lateout("r9") _, lateout("r8") _, lateout("rdi") _, lateout("rsi") _, lateout("rdx") _, lateout("rcx") _, options(nostack))
    };

    // 1 result
    ($syscall:expr => $result:ident) => {
        core::arch::asm!("int 0x80", in("rdi") $syscall, lateout("rax") $result, lateout("r11") _, lateout("r10") _, lateout("r9") _, lateout("r8") _, lateout("rdi") _, lateout("rsi") _, lateout("rdx") _, lateout("rcx") _, options(nostack))
    };
    ($syscall:expr, $arg1:expr => $result:ident) => {
        core::arch::asm!("int 0x80", in("rdi") $syscall, in("rsi") $arg1, lateout("rax") $result, lateout("r11") _, lateout("r10") _, lateout("r9") _, lateout("r8") _, lateout("rdi") _, lateout("rsi") _, lateout("rdx") _, lateout("rcx") _, options(nostack))
    };
    ($syscall:expr, $arg1:expr, $arg2:expr => $result:ident) => {
        core::arch::asm!("int 0x80", in("rdi") $syscall, in("rsi") $arg1, in("rdx") $arg2, lateout("rax") $result, lateout("r11") _, lateout("r10") _, lateout("r9") _, lateout("r8") _, lateout("rdi") _, lateout("rsi") _, lateout("rdx") _, lateout("rcx") _, options(nostack))
    };
    ($syscall:expr, $arg1:expr, $arg2:expr, $arg3:expr => $result:ident) => {
        core::arch::asm!("int 0x80", in("rdi") $syscall, in("rsi") $arg1, in("rdx") $arg2, in("rcx") $arg3, lateout("rax") $result, lateout("r11") _, lateout("r10") _, lateout("r9") _, lateout("r8") _, lateout("rdi") _, lateout("rsi") _, lateout("rdx") _, lateout("rcx") _, options(nostack))
    };
    ($syscall:expr, $arg1:expr, $arg2:expr, $arg3:expr, $arg4:expr => $result:ident) => {
        core::arch::asm!("int 0x80", in("rdi") $syscall, in("rsi") $arg1, in("rdx") $arg2, in("rcx") $arg3, in("r8") $arg4, lateout("rax") $result, lateout("r11") _, lateout("r10") _, lateout("r9") _, lateout("r8") _, lateout("rdi") _, lateout("rsi") _, lateout("rdx") _, lateout("rcx") _, options(nostack))
    };
    ($syscall:expr, $arg1:expr, $arg2:expr, $arg3:expr, $arg4:expr, $arg5:expr => $result:ident) => {
        core::arch::asm!("int 0x80", in("rdi") $syscall, in("rsi") $arg1, in("rdx") $arg2, in("rcx") $arg3, in("r8") $arg4, in("r9") $arg5, lateout("rax") $result, lateout("r11") _, lateout("r10") _, lateout("r9") _, lateout("r8") _, lateout("rdi") _, lateout("rsi") _, lateout("rdx") _, lateout("rcx") _, options(nostack))
    };
}

#[cfg(all(not(feature = "kernel"), not(feature = "iret")))]
#[macro_export]
macro_rules! make_syscall {
    // No result
    ($syscall:expr) => {
        core::arch::asm!("syscall", in("rdi") $syscall, lateout("rax") _, lateout("r15") _, lateout("r14") _, lateout("r13") _, lateout("r11") _, lateout("r10") _, lateout("r9") _, lateout("r8") _, lateout("rdi") _, lateout("rsi") _, lateout("rdx") _, lateout("rcx") _, options(nostack))
    };
    ($syscall:expr, $arg1:expr) => {
        core::arch::asm!("syscall", in("rdi") $syscall, in("rsi") $arg1, lateout("rax") _, lateout("r15") _, lateout("r14") _, lateout("r13") _, lateout("r11") _, lateout("r10") _, lateout("r9") _, lateout("r8") _, lateout("rdi") _, lateout("rsi") _, lateout("rdx") _, lateout("rcx") _, options(nostack))
    };
    ($syscall:expr, $arg1:expr, $arg2:expr) => {
        core::arch::asm!("syscall", in("rdi") $syscall, in("rsi") $arg1, in("rdx") $arg2, lateout("rax") _, lateout("r15") _, lateout("r14") _, lateout("r13") _, lateout("r11") _, lateout("r10") _, lateout("r9") _, lateout("r8") _, lateout("rdi") _, lateout("rsi") _, lateout("rdx") _, lateout("rcx") _, options(nostack))
    };
    ($syscall:expr, $arg1:expr, $arg2:expr, $arg3:expr) => {
        core::arch::asm!("syscall", in("rdi") $syscall, in("rsi") $arg1, in("rdx") $arg2, in("r10") $arg3, lateout("rax") _, lateout("r15") _, lateout("r14") _, lateout("r13") _, lateout("r11") _, lateout("r10") _, lateout("r9") _, lateout("r8") _, lateout("rdi") _, lateout("rsi") _, lateout("rdx") _, lateout("rcx") _, options(nostack))
    };

    // 1 result
    ($syscall:expr => $result:ident) => {
        core::arch::asm!("syscall", in("rdi") $syscall, lateout("rax") $result, lateout("r15") _, lateout("r14") _, lateout("r13") _, lateout("r11") _, lateout("r10") _, lateout("r9") _, lateout("r8") _, lateout("rdi") _, lateout("rsi") _, lateout("rdx") _, lateout("rcx") _, options(nostack))
    };
    ($syscall:expr, $arg1:expr => $result:ident) => {
        core::arch::asm!("syscall", in("rdi") $syscall, in("rsi") $arg1, lateout("rax") $result, lateout("r15") _, lateout("r14") _, lateout("r13") _, lateout("r11") _, lateout("r10") _, lateout("r9") _, lateout("r8") _, lateout("rdi") _, lateout("rsi") _, lateout("rdx") _, lateout("rcx") _, options(nostack))
    };
    ($syscall:expr, $arg1:expr, $arg2:expr => $result:ident) => {
        core::arch::asm!("syscall", in("rdi") $syscall, in("rsi") $arg1, in("rdx") $arg2, lateout("rax") $result, lateout("r15") _, lateout("r14") _, lateout("r13") _, lateout("r11") _, lateout("r10") _, lateout("r9") _, lateout("r8") _, lateout("rdi") _, lateout("rsi") _, lateout("rdx") _, lateout("rcx") _, options(nostack))
    };
    ($syscall:expr, $arg1:expr, $arg2:expr, $arg3:expr => $result:ident) => {
        core::arch::asm!("syscall", in("rdi") $syscall, in("rsi") $arg1, in("rdx") $arg2, in("r10") $arg3, lateout("rax") $result, lateout("r15") _, lateout("r14") _, lateout("r13") _, lateout("r11") _, lateout("r10") _, lateout("r9") _, lateout("r8") _, lateout("rdi") _, lateout("rsi") _, lateout("rdx") _, lateout("rcx") _, options(nostack))
    };
    ($syscall:expr, $arg1:expr, $arg2:expr, $arg3:expr, $arg4:expr => $result:ident) => {
        core::arch::asm!("syscall", in("rdi") $syscall, in("rsi") $arg1, in("rdx") $arg2, in("r10") $arg3, in("r8") $arg4, lateout("rax") $result, lateout("r15") _, lateout("r14") _, lateout("r13") _, lateout("r11") _, lateout("r10") _, lateout("r9") _, lateout("r8") _, lateout("rdi") _, lateout("rsi") _, lateout("rdx") _, lateout("rcx") _, options(nostack))
    };
    ($syscall:expr, $arg1:expr, $arg2:expr, $arg3:expr, $arg4:expr, $arg5:expr => $result:ident) => {
        core::arch::asm!("syscall", in("rdi") $syscall, in("rsi") $arg1, in("rdx") $arg2, in("r10") $arg3, in("r8") $arg4, in("r9") $arg5, lateout("rax") $result, lateout("r15") _, lateout("r14") _, lateout("r13") _, lateout("r11") _, lateout("r10") _, lateout("r9") _, lateout("r8") _, lateout("rdi") _, lateout("rsi") _, lateout("rdx") _, lateout("rcx") _, options(nostack))
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

pub fn sleep(ms: u64) -> u64 {
    let real: u64;
    unsafe { make_syscall!(SLEEP, ms => real) }
    real
}

pub fn get_pid() -> ProcessID {
    unsafe {
        let pid: u64;
        make_syscall!(GET_PID => pid);
        ProcessID(pid)
    }
}
