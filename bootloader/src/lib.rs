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

pub const KERNEL_RECLAIM: MemoryType = MemoryType::custom(0x80000000);
pub const KERNEL_MEMORY: MemoryType = MemoryType::custom(0x80000001);

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
#[derive(Debug)]
#[repr(C)]
pub struct BootInfo {
    pub uefi_runtime_table: u64,
    pub gop: gop::GopInfo,
    pub mmap: MemoryMapEntrySlice,
    pub kernel_start: u64,
    pub kernel_pages: u64,
}

#[derive(Debug)]
#[repr(C)]
pub struct MemoryMapEntrySlice {
    ptr: *mut MemoryMapEntry,
    capacity: usize,
    len: usize,
}

impl MemoryMapEntrySlice {
    pub unsafe fn new(ptr: *mut MemoryMapEntry, capacity: usize) -> Self {
        Self {
            ptr,
            capacity,
            len: 0,
        }
    }

    pub fn push(&mut self, e: MemoryMapEntry) {
        assert!(self.capacity >= self.len + 1);
        unsafe { *self.ptr.add(self.len) = e };
        self.len += 1;
    }

    pub fn get(&self, idx: usize) -> &MemoryMapEntry {
        assert!(idx < self.len);
        unsafe { &*self.ptr.add(idx) }
    }

    pub fn iter(&self) -> MemoryMapEntrySliceIter {
        MemoryMapEntrySliceIter {
            ptr: self.ptr,
            len: self.len,
            pos: 0,
        }
    }
}

#[derive(Debug, Clone, Copy)]
pub struct MemoryMapEntrySliceIter {
    ptr: *const MemoryMapEntry,
    len: usize,
    pos: usize,
}

impl Iterator for MemoryMapEntrySliceIter {
    type Item = *const MemoryMapEntry;

    fn next(&mut self) -> Option<Self::Item> {
        if self.pos + 1 == self.len {
            None
        } else {
            self.pos += 1;
            Some(unsafe { self.ptr.add(self.pos - 1) })
        }
    }
}

#[derive(Debug, Clone, Copy)]
#[repr(C)]
pub struct MemoryMapEntry {
    pub class: MemoryClass,
    /// Starting physical address.
    pub phys_start: u64,
    /// Number of 4 KiB pages contained in this range.
    pub page_count: u64,
}

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
#[repr(C)]
pub enum MemoryClass {
    Free,
    KernelReclaim,
    KernelMemory,
    Unusable,
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
