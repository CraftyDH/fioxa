use alloc::collections::BTreeMap;
use bootloader::gop::GopInfo;
use core::time::Duration;
use kernel_sys::syscall::{
    sys_handle_drop, sys_map, sys_process_spawn_thread, sys_sleep, sys_vmo_mmap_create,
};
use kernel_sys::types::VMMapFlags;
use spin::Once;

#[derive(Clone, Copy)]
pub struct Pos {
    pub x: usize,
    pub y: usize,
}

pub const CHAR_HEIGHT: usize = 16;
pub const CHAR_WIDTH: usize = 8;

pub struct Screen<'a> {
    pub gop: GopInfo,
    pub font: PSF1Font<'a>,
    pub unicode_table: BTreeMap<char, usize>,
}

impl Screen<'_> {
    pub fn update_cell(&mut self, cell: &Cell, x: usize, y: usize) {
        let mut addr: usize = *self.unicode_table.get(&cell.chr).unwrap_or(&0);

        // let mut addr = chr as usize;
        // if addr > 255 {
        //     addr = 0
        // }
        addr *= 16;

        let ptr = self.gop.buffer.get_mut();

        let xoff = x * CHAR_WIDTH;
        let yoff = y * CHAR_HEIGHT;

        for y in yoff..(yoff + 16) {
            let glyph = self.font.glyph_buffer[addr];
            for x in xoff..(xoff + 8) {
                // Fancy math to check if bit is on.
                let color = if (glyph & (0b10_000_000 >> (x - xoff))) > 0 {
                    cell.fg
                } else {
                    cell.bg
                };
                let loc = (x + (y * self.gop.stride)) * 4;
                unsafe { core::ptr::write_volatile(ptr.add(loc) as *mut u32, color) }
            }
            addr += 1;
        }
    }

    pub fn draw_cursor(&mut self, mut pos: Pos, colour: u32, cursor: &[u16]) {
        if pos.x > self.gop.horizonal - 16 {
            pos.x = self.gop.horizonal - 16
        }

        if pos.y > self.gop.vertical - 16 {
            pos.y = self.gop.vertical - 16
        }

        for (y, line) in cursor.iter().enumerate().take(16) {
            for x in 0..16 {
                if (line & (0b10_000_000_000_000 >> x)) > 0 {
                    let loc = ((x + pos.x) + ((y + pos.y) * self.gop.stride)) * 4;
                    unsafe {
                        core::ptr::write_volatile(
                            self.gop.buffer.get_mut().add(loc) as *mut u32,
                            colour,
                        )
                    }
                }
            }
        }
    }
}

pub static WRITER: Once<Spinlock<Writer>> = Once::new();

#[macro_export]
macro_rules! colour {
    ($colour:expr) => {
        $crate::gop::WRITER.lock().set_colour($colour)
    };
}

use crate::BOOT_INFO;
use crate::mutex::Spinlock;
use crate::terminal::{Cell, Writer};

use super::mouse::monitor_cursor_task;
use super::psf1::PSF1Font;

fn redraw_screen_task() {
    let writer = WRITER.get().unwrap();
    // TODO: Can we VSYNC this? Could stop the tearing.
    loop {
        writer.lock().redraw_if_needed();
        // rate limit redraw
        sys_sleep(Duration::from_millis(16));
    }
}

pub fn gop_entry() {
    let gop = unsafe { &(*BOOT_INFO).gop };

    let fb_ptr = unsafe { *gop.buffer.as_ptr() as usize };
    let fb_base = fb_ptr & !0xFFF;
    let fb_top = (fb_base + gop.buffer_size + 0xFFF) & !0xFFF;

    unsafe {
        let handle = sys_vmo_mmap_create(fb_base as *mut (), fb_top - fb_base);
        sys_map(
            Some(handle),
            VMMapFlags::WRITEABLE,
            fb_base as *mut (),
            fb_top - fb_base,
        )
        .unwrap();
        sys_handle_drop(handle).assert_ok();
    };

    sys_process_spawn_thread(monitor_cursor_task);
    redraw_screen_task();
}
