use alloc::collections::BTreeMap;
use bootloader::gop::GopInfo;
use bootloader::psf1::{PSF1Font, PSF1_FONT_NULL};
use core::fmt::Write;
use core::sync::atomic::AtomicPtr;
use lazy_static::lazy_static;

#[derive(Clone, Copy)]
pub struct Pos {
    pub x: usize,
    pub y: usize,
}

pub struct Writer {
    pub pos: Pos,
    pub gop: GopInfo,
    pub font: PSF1Font,
    pub fg_colour: u32,
    pub bg_colour: u32,
    pub unicode_table: Option<BTreeMap<char, usize>>,
}

impl Writer {
    pub fn set_gop(&mut self, gop: GopInfo, font: PSF1Font) {
        // self.pos = Pos { x: 0, y: 0 };
        self.gop = gop;

        self.font = font;
    }

    /// Requires Allocation to be enabled first!
    pub fn generate_unicode_mapping(&mut self, unicode_buffer: &[u8]) {
        let mut unicode_table: BTreeMap<char, usize> = BTreeMap::new();

        let mut index = 0;
        for byte_index in (0..unicode_buffer.len()).step_by(2) {
            let unicode_byte =
                (unicode_buffer[byte_index] as u16) | (unicode_buffer[byte_index + 1] as u16) << 8;

            if unicode_byte == 0xFFFF {
                index += 1;
            } else {
                unicode_table.insert(char::from_u32(unicode_byte.into()).unwrap(), index);
            }
        }

        self.unicode_table = Some(unicode_table)
    }

    pub fn put_char(&mut self, colour: u32, chr: char, xoff: usize, yoff: usize) {
        // let addr = ('A' as usize) * (font.psf1_header.charsize as usize);
        let mut addr: usize;
        if let Some(unicode) = &self.unicode_table {
            addr = *unicode.get(&chr).unwrap_or(&0);
        } else {
            addr = chr as usize;
            if addr > 255 {
                addr = 0
            }
        }
        addr *= 16;

        // let glyphbuf = font.glyph_buffer;

        for y in yoff..(yoff + 16) {
            let glyph = self.font.glyph_buffer[addr];
            for x in xoff..(xoff + 8) {
                // Fancy math to check if bit is on.
                if (glyph & (0b10_000_000 >> (x - xoff))) > 0 {
                    let loc = (x + (y * self.gop.stride)) * 4;
                    unsafe {
                        core::ptr::write(self.gop.buffer.get_mut().add(loc) as *mut u32, colour)
                    }
                }
            }
            addr += 1;
        }
    }

    pub fn fill_screen(&mut self, colour: u32) {
        unsafe {
            let buf = self.gop.buffer.get_mut();
            for y in 0..self.gop.vertical {
                for x in 0..self.gop.horizonal {
                    core::ptr::write_volatile(
                        buf.add((y * self.gop.stride + x) * 4) as *mut u32,
                        colour,
                    )
                }
            }
        }
    }

    #[allow(dead_code)]
    pub fn set_colour(&mut self, colour: u32) {
        // Get pointer to then change colour
        self.fg_colour = colour;
    }

    pub fn write_byte(&mut self, chr: char) {
        match chr {
            '\n' => {
                self.pos.x = 0;
                self.pos.y += 16;
            }
            // Backspace control character
            '\x08' => {
                // Check bounds
                if self.pos.x < 8 {
                    self.pos.x = self.gop.horizonal - 8;
                    if self.pos.y >= 16 {
                        self.pos.y -= 16
                    }
                } else {
                    self.pos.x -= 8;
                }

                let buf = self.gop.buffer.get_mut();
                unsafe {
                    for y in self.pos.y..(self.pos.y + 16) {
                        for x in self.pos.x..(self.pos.x + 8) {
                            core::ptr::write_volatile(
                                buf.add((y * self.gop.stride + x) * 4) as *mut u32,
                                self.bg_colour,
                            )
                        }
                    }
                }
            }
            '\u{7F}' => {
                if self.pos.x >= self.gop.horizonal - 8 {
                    return;
                }
                let buf = self.gop.buffer.get_mut();
                unsafe {
                    for y in self.pos.y..(self.pos.y + 16) {
                        for x in self.pos.x..(self.pos.x + 8) {
                            core::ptr::write_volatile(
                                buf.add((y * self.gop.stride + x) * 4) as *mut u32,
                                self.bg_colour,
                            )
                        }
                    }
                }
            }
            chr => {
                self.put_char(self.fg_colour, chr, self.pos.x, self.pos.y);
                self.pos.x += 8
            }
        }
        self.check_bounds();
    }

    pub fn write_string(&mut self, s: &str) {
        for chr in s.chars() {
            self.write_byte(chr);
        }
    }

    pub fn draw_cursor(&mut self, mut pos: Pos, colour: u32, cursor: &[u16]) {
        if pos.x > self.gop.horizonal - 16 {
            pos.x = self.gop.horizonal - 16
        }

        if pos.y > self.gop.vertical - 16 {
            pos.y = self.gop.vertical - 16
        }

        for y in 0..16 {
            let line = cursor[y];
            for x in 0..16 {
                if (line & (0b10_000_000_000_000 >> x)) > 0 {
                    let loc = ((x + pos.x) + ((y + pos.y) * self.gop.stride)) * 4;
                    unsafe {
                        core::ptr::write(self.gop.buffer.get_mut().add(loc) as *mut u32, colour)
                    }
                }
            }
        }
    }

    fn check_bounds(&mut self) {
        // Check if next character will excede width
        let res = (self.gop.horizonal, self.gop.vertical);
        if self.pos.x + 8 > res.0 {
            self.pos.x = 0;
            self.pos.y += 16;
        }
        // Check if next line will excede height
        if self.pos.y + 16 > res.1 {
            // Start copying line line in
            let size_of_line = 16 * 4 * self.gop.stride;
            // As such copy the hole buffer less that line
            let size_of_buffer = self.gop.buffer_size - size_of_line;

            let buf = self.gop.buffer.get_mut();
            unsafe {
                // Copy memory from bottom to top (aka scroll)
                core::ptr::copy(buf.offset(size_of_line as isize), *buf, size_of_buffer);

                // Clear the bottom line by writing zeros
                core::ptr::write_bytes(buf.offset(size_of_buffer as isize), 0, size_of_line);
            }

            self.pos.y -= 16;
            self.pos.x = 0
        }
    }
}

impl core::fmt::Write for Writer {
    fn write_str(&mut self, s: &str) -> core::fmt::Result {
        self.write_string(s);
        Ok(())
    }
}

use spin::Mutex;

lazy_static! {
    pub static ref WRITER: Mutex<Writer> = Mutex::new(Writer {
        pos: Pos { x: 0, y: 0 },
        gop: GopInfo {
            buffer: AtomicPtr::default(),
            buffer_size: 0,
            horizonal: 0,
            vertical: 0,
            stride: 0,
            pixel_format: uefi::proto::console::gop::PixelFormat::Rgb
        },
        font: PSF1_FONT_NULL,
        unicode_table: None,
        fg_colour: 0xFF_FF_FF,
        bg_colour: 0x00_00_00,
    });
}

#[macro_export]
macro_rules! println {
    () => (print!("\n"));
    ($($arg:tt)*) => (print!("{}\n", format_args!($($arg)*)));
}

#[macro_export]
macro_rules! print {
    ($($arg:tt)*) => ($crate::screen::gop::_print(format_args!($($arg)*)));
}

#[macro_export]
macro_rules! colour {
    ($colour:expr) => {
        $crate::gop::WRITER.lock().set_colour($colour)
    };
}

use core::fmt::Arguments;

#[doc(hidden)]
pub fn _print(args: Arguments) {
    WRITER
        .try_lock()
        .and_then(|mut w| w.write_fmt(args).ok())
        .unwrap();
}
