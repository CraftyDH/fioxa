#![no_std]
#![feature(error_in_core)]

use syscall::sleep;

#[macro_use]
extern crate alloc;

pub mod disk;
pub mod elf;
pub mod fs;
pub mod ids;
pub mod input;
pub mod net;
pub mod pci;
pub mod service;
pub mod syscall;

/// Calls f, backing off by 1ms doubling each fail attempt
pub fn backoff_sleep<R>(mut f: impl FnMut() -> Option<R>) -> R {
    let mut time = 1;
    loop {
        if let Some(r) = f() {
            return r;
        }
        sleep(time);
        time *= 2;
    }
}
