#![no_std]
#![feature(fn_traits)]
#![feature(box_into_inner)]
#![feature(try_trait_v2)]

#[macro_use]
extern crate alloc;

pub mod channel;
pub mod disk;
pub mod elf;
pub mod fs;
pub mod handle;
pub mod input;
pub mod interrupt;
pub mod ipc;
pub mod message;
pub mod net;
pub mod pci;
pub mod port;
pub mod process;
pub mod service;

pub use kernel_sys as sys;
pub use rkyv;

use core::time::Duration;

use kernel_sys::syscall::sys_sleep;
pub use num_derive;
pub use num_traits;

/// Calls f, backing off by 1ms adding 1ms each time maxing at 10ms
pub fn backoff_sleep<R>(mut f: impl FnMut() -> Option<R>) -> R {
    let mut time = 1;
    loop {
        if let Some(r) = f() {
            return r;
        }
        sys_sleep(Duration::from_millis(time));
        // max at 10ms
        time = 10.max(time + 1);
    }
}
