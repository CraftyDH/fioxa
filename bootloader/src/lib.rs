#![no_std]
#![feature(slice_pattern)]

#[macro_use]
extern crate log;

pub mod fs;
pub mod gop;
pub mod kernel;
pub mod paging;

pub use uefi;

use core::{mem::size_of, slice};

use uefi::{prelude::BootServices, table::boot::MemoryType};

/// Struct that frees the memory of the buffer once this object is dropped
pub struct OwnedBuffer<'s, 'b> {
    pub bt: &'s BootServices,
    pub buf: &'b mut [u8],
}

impl<'s, 'b> OwnedBuffer<'s, 'b> {
    pub fn new(bt: &'s BootServices, size: usize) -> Self {
        let buf = unsafe { get_buffer(bt, size) };
        Self { bt, buf }
    }

    pub fn from_buf(bt: &'s BootServices, buf: &'b mut [u8]) -> Self {
        Self { bt: bt, buf: buf }
    }
}

impl<'s, 'b> Drop for OwnedBuffer<'s, 'b> {
    fn drop(&mut self) {
        // Calls the uefi free pool method which frees the memory of the buffer
        self.bt.free_pool(self.buf.as_ptr() as *mut u8).unwrap();
    }
}

pub unsafe fn get_buffer<'b, T>(bt: &BootServices, length: usize) -> &'b mut [T] {
    let ptr = bt
        .allocate_pool(MemoryType::LOADER_DATA, size_of::<T>() * length)
        .unwrap();
    slice::from_raw_parts_mut(ptr as *mut T, length)
}

pub unsafe fn get_buffer_as_type<'b, T>(bt: &BootServices) -> &'b mut T {
    let ptr = bt
        .allocate_pool(MemoryType::LOADER_DATA, size_of::<T>())
        .unwrap();
    &mut *(ptr as *mut T)
}

/// The struct that is passed from bootloader to the kernel
pub struct BootInfo<'f> {
    pub uefi_runtime_table: u64,
    pub gop: gop::GopInfo,
    pub font: &'f [u8],
    pub mmap_buf: *mut u8,
    pub mmap_entry_size: usize,
    pub mmap_len: usize,
    pub rsdp_address: usize,
    pub kernel_start: u64,
    pub kernel_pages: u64,
}

pub type EntryPoint = fn(*const BootInfo) -> !;

#[macro_export]
macro_rules! entry_point {
    ($path:path) => {
        #[export_name = "_start"]
        // We are reciecing the call from UEFI which is win64 calling
        pub extern "C" fn bootstrap(info: *const bootloader::BootInfo) -> ! {
            let f: bootloader::EntryPoint = $path;

            f(info)
        }
    };
}
