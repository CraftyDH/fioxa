#![no_std]
#![no_main]

use alloc::vec::Vec;
use userspace::syscall::echo;

extern crate alloc;
#[macro_use]
extern crate userspace;
extern crate userspace_bumpalloc;

#[export_name = "_start"]
pub extern "C" fn main() {
    echo(123);
    let mut x = Vec::with_capacity(0x1000);
    for i in 0..10 {
        x.push(i);
    }
    echo(123);
    for i in x {
        println!("Hello {i}");
    }
}

#[panic_handler]
fn panic(i: &core::panic::PanicInfo) -> ! {
    println!("{}", i);
    loop {}
}
