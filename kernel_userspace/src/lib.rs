#![no_std]
#![feature(fn_traits)]
#![feature(box_into_inner)]

use syscall::sleep;

#[macro_use]
extern crate alloc;

pub mod disk;
pub mod elf;
pub mod event;
pub mod fs;
pub mod ids;
pub mod input;
pub mod message;
pub mod net;
pub mod object;
pub mod pci;
pub mod process;
pub mod service;
pub mod socket;
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

pub const INT_KB: usize = 0;
pub const INT_MOUSE: usize = 1;
pub const INT_PCI: usize = 2;
