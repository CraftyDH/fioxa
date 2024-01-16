use core::{mem::size_of, slice};

use thiserror::Error;

pub const PSF1_MAGIC: [u8; 2] = [0x36, 0x04];

#[derive(Debug, Clone, Copy)]
pub struct PSF1FontHeader {
    pub magic: [u8; 2],
    pub mode_512: u8,
    pub charsize: u8,
}

#[derive(Debug, Clone, Copy)]
pub struct PSF1Font<'a> {
    pub psf1_header: &'a PSF1FontHeader,
    pub glyph_buffer: &'a [u8],
    pub unicode_buffer: &'a [u8],
}

const PSF1_HEADER_SIZE: usize = size_of::<PSF1FontHeader>();

#[derive(Debug, Error)]
#[error("psf1 font invalid header, expected ({PSF1_MAGIC:?}), found ({0:?})")]
pub struct LoadFontInvMagic([u8; 2]);

pub fn load_psf1_font(file: &[u8]) -> Result<PSF1Font<'_>, LoadFontInvMagic> {
    let psf1_header = unsafe { &*(file.as_ptr() as *const PSF1FontHeader) };

    if psf1_header.magic != PSF1_MAGIC {
        return Err(LoadFontInvMagic(psf1_header.magic));
    }

    let mut glyph_buffer_size = (psf1_header.charsize as usize) * 256;
    if psf1_header.mode_512 == 1 {
        // 512 glyph mode
        glyph_buffer_size *= 2;
    }

    let psf1_font =
        unsafe { slice::from_raw_parts(file.as_ptr().add(PSF1_HEADER_SIZE), glyph_buffer_size) };

    let unicode_table_buffer = unsafe {
        slice::from_raw_parts(
            file.as_ptr().add(PSF1_HEADER_SIZE + glyph_buffer_size),
            file.len() - PSF1_HEADER_SIZE - glyph_buffer_size,
        )
    };

    Ok(PSF1Font {
        psf1_header,
        glyph_buffer: psf1_font,
        unicode_buffer: unicode_table_buffer,
    })
}
