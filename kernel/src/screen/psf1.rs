use core::{mem::size_of, slice};

use crate::paging::virt_addr_for_phys;

pub const PSF1_MAGIC: [u8; 2] = [0x36, 0x04];

#[derive(Debug, Clone, Copy)]
pub struct PSF1FontHeader {
    pub magic: [u8; 2],
    pub mode_512: u8,
    pub charsize: u8,
}

#[derive(Debug, Clone, Copy)]
pub struct PSF1Font {
    pub psf1_header: &'static PSF1FontHeader,
    pub glyph_buffer: &'static [u8],
    pub unicode_buffer: &'static [u8],
}

pub const PSF1_FONT_NULL: PSF1Font = PSF1Font {
    psf1_header: &PSF1FontHeader {
        magic: PSF1_MAGIC,
        mode_512: 0,
        charsize: 0,
    },
    glyph_buffer: &[0],
    unicode_buffer: &[0],
};

const PSF1_HEADER_SIZE: usize = size_of::<PSF1FontHeader>();

pub fn load_psf1_font(file: &[u8]) -> PSF1Font {
    let psf1_header =
        unsafe { &*(virt_addr_for_phys(file.as_ptr() as u64) as *const PSF1FontHeader) };

    if psf1_header.magic != PSF1_MAGIC {
        panic!("PSF1 FONT not valid");
    }

    let mut glyph_buffer_size = (psf1_header.charsize as usize) * 256;
    if psf1_header.mode_512 == 1 {
        // 512 glyph mode
        glyph_buffer_size *= 2;
    }

    let psf1_font = unsafe {
        slice::from_raw_parts(
            virt_addr_for_phys(file.as_ptr() as u64 + PSF1_HEADER_SIZE as u64) as *mut u8,
            glyph_buffer_size,
        )
    };

    let unicode_table_buffer = unsafe {
        slice::from_raw_parts(
            virt_addr_for_phys(
                file.as_ptr() as u64 + PSF1_HEADER_SIZE as u64 + glyph_buffer_size as u64,
            ) as *const u8,
            file.len() - PSF1_HEADER_SIZE - glyph_buffer_size,
        )
    };

    PSF1Font {
        psf1_header,
        glyph_buffer: psf1_font,
        unicode_buffer: unicode_table_buffer,
    }
}
