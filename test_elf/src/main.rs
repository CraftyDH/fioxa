#![no_std]
#![no_main]

#[export_name = "_start"]
pub extern "C" fn main() {
    for i in 0.. {
        echo(i);
        yield_now()
    }
}

#[panic_handler]
fn panic(_: &core::panic::PanicInfo) -> ! {
    loop {}
}

pub const SYSCALL_ADDR: usize = 0x80;
const ECHO: usize = 0;
const YIELD_NOW: usize = 1;

unsafe fn syscall1(mut syscall_number: usize, arg1: usize) -> usize {
    core::arch::asm!("int 0x80", inout("rax") syscall_number, in("r8") arg1, options(nostack));
    syscall_number
}

/// Syscall test
/// Will return number passed as arg1
pub fn echo(number: usize) -> usize {
    unsafe { syscall1(ECHO, number) }
}

pub fn yield_now() {
    unsafe { syscall1(YIELD_NOW, 0) };
}
