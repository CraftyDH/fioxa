use alloc::collections::BTreeMap;
use alloc::string::String;
use alloc::vec::Vec;
use bootloader::gop::GopInfo;
use conquer_once::spin::OnceCell;
use core::fmt::Write;
use core::ops::ControlFlow;
use core::time::Duration;
use kernel_sys::syscall::{sys_process_spawn_thread, sys_sleep};
use kernel_sys::types::SyscallResult;
use kernel_userspace::service::Service;

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

        for y in 0..16 {
            let line = cursor[y];
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

pub static WRITER: OnceCell<Spinlock<Writer>> = OnceCell::uninit();

#[macro_export]
macro_rules! colour {
    ($colour:expr) => {
        $crate::gop::WRITER.lock().set_colour($colour)
    };
}

use crate::cpu_localstorage::CPULocalStorageRW;
use crate::mutex::Spinlock;
use crate::paging::offset_map::get_gop_range;
use crate::paging::MemoryMappingFlags;
use crate::scheduling::with_held_interrupts;
use crate::terminal::{Cell, Writer};
use crate::BOOT_INFO;

use super::mouse::monitor_cursor_task;
use super::psf1::PSF1Font;

pub fn monitor_stdout_task() {
    let mut data_buf = Vec::with_capacity(0x1000);
    let mut service = Service::new(
        "STDOUT",
        || (),
        |handle, ()| {
            match handle.read::<0>(&mut data_buf, false, false) {
                Ok(_) => (),
                e => {
                    warn!("error recv {e:?}");
                    return ControlFlow::Break(());
                }
            };
            let s = String::from_utf8_lossy(&data_buf);
            with_held_interrupts(|| {
                let mut w = WRITER.get().unwrap().lock();
                w.write_str(&s).unwrap();
            });
            match handle.write(&[], &[]) {
                SyscallResult::Ok => ControlFlow::Continue(()),
                SyscallResult::ChannelClosed | SyscallResult::ChannelFull => ControlFlow::Break(()),
                e => {
                    warn!("error send {e:?}");
                    return ControlFlow::Break(());
                }
            }
        },
    );
    service.run();
}

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
    // Map the GOP range
    with_held_interrupts(|| unsafe {
        let gop = get_gop_range(&(*BOOT_INFO).gop);
        let proc = CPULocalStorageRW::get_current_task().process();
        let mut mem = proc.memory.lock();

        mem.page_mapper
            .insert_mapping_at_set(gop.0, gop.1, MemoryMappingFlags::WRITEABLE)
            .unwrap();
    });

    sys_process_spawn_thread(monitor_cursor_task);
    sys_process_spawn_thread(redraw_screen_task);
    monitor_stdout_task();
}
