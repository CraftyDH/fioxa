// use bootloader::psf1::{PSF1Font, PSF1FontHeader};

// const PSF1_MAGIC: [u8; 2] = [0x36, 0x04];

// struct PSF1FontHeader {
//     magic: [u8; 2],
//     mode: u8,
//     charsize: u8,
// }

// pub struct PSF1Font<'a> {
//     psf1_header: PSF1FontHeader,
//     pub glyph_buffer: &'a [u8],
// }

// A Null psf1 font to use in place of the real PSF1 Font
// pub const PSF1_FONT_NULL: PSF1Font = PSF1Font {
//     psf1_header: PSF1FontHeader {
//         magic: PSF1_MAGIC,
//         mode: 0,
//         charsize: 0,
//     },
//     glyph_buffer: &[0u8],
// };
