#![no_std]
#![feature(error_in_core)]
#![feature(fn_traits)]
#![feature(box_into_inner)]

use syscall::sleep;

#[macro_use]
extern crate alloc;

pub mod disk;
pub mod elf;
pub mod fs;
pub mod ids;
pub mod input;
pub mod message;
pub mod net;
pub mod pci;
pub mod service;
pub mod syscall;

pub use num_derive;
pub use num_traits;

/// Calls f, backing off by 1ms adding 1ms each time maxing at 10ms
pub fn backoff_sleep<R>(mut f: impl FnMut() -> Option<R>) -> R {
    let mut time = 1;
    loop {
        if let Some(r) = f() {
            return r;
        }
        sleep(time);
        // max at 10ms
        time = 10.max(time + 1);
    }
}
