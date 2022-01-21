#![no_std]

use core::sync::atomic::AtomicPtr;

extern crate alloc;

use uefi::{proto::console::gop::PixelFormat, table::boot::MemoryDescriptor};

pub struct BootInfo<'life> {
    pub gop: GopInfo,
    pub font: PSF1Font,
    pub mmap: &'life [MemoryDescriptor],
    pub rsdp: &'life RSDP2,
}

pub type EntryPoint = fn(BootInfo) -> !;

#[macro_export]
macro_rules! entry_point {
    ($path:path) => {
        #[export_name = "_start"]
        // We are calling from UEFI expect win64 calling
        pub extern "win64" fn bootstrap(info: types::BootInfo) -> ! {
            let f: types::EntryPoint = $path;

            f(info)
        }
    };
}
#[derive(Debug)]

pub struct GopInfo {
    pub buffer: AtomicPtr<u8>,
    pub buffer_size: usize,
    pub horizonal: usize,
    pub vertical: usize,
    pub stride: usize,
    pub pixel_format: PixelFormat,
}

pub const PSF1_MAGIC: [u8; 2] = [0x36, 0x04];

#[derive(Debug, Clone, Copy)]
pub struct PSF1FontHeader {
    pub magic: [u8; 2],
    pub mode_512: u8,
    pub charsize: u8,
}

#[derive(Debug, Clone, Copy)]
pub struct PSF1Font {
    pub psf1_header: PSF1FontHeader,
    pub glyph_buffer: &'static [u8],
    pub unicode_buffer: &'static [u8],
}

pub const PSF1_FONT_NULL: PSF1Font = PSF1Font {
    psf1_header: PSF1FontHeader {
        magic: PSF1_MAGIC,
        mode_512: 0,
        charsize: 0,
    },
    glyph_buffer: &[0u8],
    unicode_buffer: &[0u8],
};

// ACPI
#[repr(C, packed)]
pub struct RSDP2 {
    pub signature: [u8; 8],
    pub checksum: u8,
    pub oem_id: [u8; 6],
    pub revision: u8,
    pub rsdt_address: u32,
    pub length: u32,
    pub xsdt_address: u64,
    pub extended_checksum: u8,
    pub reserved: [u8; 3],
}
