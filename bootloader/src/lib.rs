#![no_std]
#![allow(clippy::missing_safety_doc)] // TODO: Fix

#[macro_use]
extern crate log;

#[macro_use]
extern crate alloc;

pub mod fs;
pub mod gop;
pub mod kernel;
pub mod paging;

pub use uefi;

/// The struct that is passed from bootloader to the kernel
#[derive(Debug)]
#[repr(C)]
pub struct BootInfo {
    pub uefi_runtime_table: u64,
    pub gop: gop::GopInfo,
    pub mmap_buf: *const u8,
    pub mmap_entry_size: usize,
    pub mmap_len: usize,
    pub kernel_start: u64,
    pub kernel_pages: u64,
}

pub type EntryPoint = unsafe fn(*const BootInfo) -> !;

#[macro_export]
macro_rules! entry_point {
    ($path:path) => {
        #[unsafe(export_name = "_start")]
        // We are reciecing the call from UEFI which is win64 calling
        pub unsafe extern "C" fn bootstrap(info: *const bootloader::BootInfo) -> ! {
            let f: bootloader::EntryPoint = $path;

            unsafe { f(info) }
        }
    };
}
