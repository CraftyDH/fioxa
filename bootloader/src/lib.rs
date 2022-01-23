#![no_std]
#![feature(asm)]
#![feature(slice_pattern)]

#[macro_use]
extern crate log;

pub mod fs;
pub mod gop;
pub mod kernel;
pub mod psf1;

use uefi::table::boot::MemoryDescriptor;

/// The struct that is passed from bootloader to the kernel
pub struct BootInfo<'life> {
    pub gop: gop::GopInfo,
    pub font: psf1::PSF1Font,
    pub mmap: &'life [MemoryDescriptor],
    pub rsdp_address: Option<usize>,
}

pub type EntryPoint = fn(*const BootInfo<'static>) -> !;

#[macro_export]
macro_rules! entry_point {
    ($path:path) => {
        #[export_name = "_start"]
        // We are reciecing the call from UEFI which is win64 calling
        pub extern "C" fn bootstrap(info: *const bootloader::BootInfo<'static>) -> ! {
            let f: bootloader::EntryPoint = $path;

            // Read the bootinfo struct
            // let boot_info = unsafe { core::ptr::read(info) };

            f(info)
        }
    };
}
