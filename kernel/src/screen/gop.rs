use alloc::collections::BTreeMap;
use bootloader::gop::GopInfo;
use conquer_once::spin::OnceCell;
use core::fmt::Write;
use core::num::NonZeroUsize;
use hashbrown::HashMap;
use kernel_userspace::event::{
    event_queue_create, event_queue_get_event, event_queue_listen, event_queue_pop,
    event_queue_unlisten, receive_event, EventCallback, EventQueueListenId,
};
use kernel_userspace::message::MessageHandle;
use kernel_userspace::object::{KernelObjectType, KernelReference};
use kernel_userspace::socket::{
    socket_accept, socket_handle_get_event, socket_listen, socket_listen_get_event, socket_recv,
    SocketEvents, SocketRecieveResult,
};
use kernel_userspace::syscall::spawn_thread;

#[derive(Clone, Copy)]
pub struct Pos {
    pub x: usize,
    pub y: usize,
}

pub struct Writer<'a> {
    pub pos: Pos,
    pub gop: GopInfo,
    pub font: PSF1Font<'a>,
    pub fg_colour: u32,
    pub bg_colour: u32,
    pub unicode_table: BTreeMap<char, usize>,
}

impl<'a> Writer<'a> {
    pub fn new(gop: GopInfo, font: PSF1Font<'a>) -> Writer {
        let unicode_buffer = &font.unicode_buffer;

        let mut unicode_table: BTreeMap<char, usize> = BTreeMap::new();

        let mut index = 0;
        for byte_index in (0..unicode_buffer.len()).step_by(2) {
            let unicode_byte =
                (unicode_buffer[byte_index] as u16) | (unicode_buffer[byte_index + 1] as u16) << 8;

            if unicode_byte == 0xFFFF {
                index += 1;
            } else {
                unicode_table.insert(
                    char::from_u32(unicode_byte.into())
                        .expect("unicode table should only have valid chars"),
                    index,
                );
            }
        }

        Self {
            pos: Pos { x: 0, y: 0 },
            gop,
            font,
            unicode_table,
            fg_colour: 0xFF_FF_FF,
            bg_colour: 0x00_00_00,
        }
    }

    pub fn put_char(&mut self, colour: u32, chr: char, xoff: usize, yoff: usize) {
        let mut addr: usize = *self.unicode_table.get(&chr).unwrap_or(&0);

        // let mut addr = chr as usize;
        // if addr > 255 {
        //     addr = 0
        // }
        addr *= 16;

        let ptr = self.gop.buffer.get_mut();

        for y in yoff..(yoff + 16) {
            let glyph = self.font.glyph_buffer[addr];
            for x in xoff..(xoff + 8) {
                // Fancy math to check if bit is on.
                if (glyph & (0b10_000_000 >> (x - xoff))) > 0 {
                    let loc = (x + (y * self.gop.stride)) * 4;
                    unsafe { core::ptr::write_volatile(ptr.add(loc) as *mut u32, colour) }
                }
            }
            addr += 1;
        }
    }

    pub fn fill_screen(&mut self, colour: u32) {
        unsafe {
            let buf = (*self.gop.buffer.get_mut()) as *mut u32;

            for y in 0..self.gop.vertical {
                for x in 0..self.gop.horizonal {
                    core::ptr::write_volatile(buf.add(y * self.gop.stride + x), colour);
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
                        core::ptr::write_volatile(
                            self.gop.buffer.get_mut().add(loc) as *mut u32,
                            colour,
                        )
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
        let max = self.gop.vertical - (self.gop.vertical % 16);
        // Check if next line will excede height
        if self.pos.y + 16 > max {
            let buf = self.gop.buffer.get_mut();
            unsafe {
                // Copy memory from bottom to top (aka scroll)
                for l in 16..max {
                    core::ptr::copy(
                        buf.offset((l * self.gop.stride * 4) as isize),
                        buf.offset(((l - 16) * self.gop.stride * 4) as isize),
                        self.gop.horizonal * 4,
                    )
                }

                // Clear the bottom line by writing zeros
                for l in (max - 16)..self.gop.vertical {
                    core::ptr::write_bytes(
                        buf.offset((l * self.gop.stride * 4) as isize),
                        0,
                        self.gop.horizonal * 4,
                    )
                }
            }

            self.pos.y -= 16;
            self.pos.x = 0
        }
    }
}

impl core::fmt::Write for Writer<'_> {
    fn write_str(&mut self, s: &str) -> core::fmt::Result {
        self.write_string(s);
        Ok(())
    }
}
pub static WRITER: OnceCell<Mutex<Writer>> = OnceCell::uninit();

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

use crate::scheduling::without_context_switch;
use core::fmt::Arguments;
use spin::mutex::Mutex;

use super::mouse::monitor_cursor_task;
use super::psf1::PSF1Font;

#[doc(hidden)]
pub fn _print(args: Arguments) {
    loop {
        // Prevent task from being scheduled away with mutex
        let res = without_context_switch(|| {
            if let Some(mut w) = WRITER.get().unwrap().try_lock() {
                w.write_fmt(args).unwrap();
                return Some(());
            }
            None
        });
        if let Some(()) = res {
            return;
        }
    }
}

struct GopMonitorInfo {
    queued: EventQueueListenId,
    #[allow(dead_code)]
    event: KernelReference,
    sock: KernelReference,
}

pub fn monitor_stdout_task() {
    let service = socket_listen("STDOUT").unwrap();
    let service_accept = socket_listen_get_event(service);

    let mut connections: HashMap<EventCallback, GopMonitorInfo> = HashMap::new();
    let event_queue = event_queue_create();
    let event_queue_ev = event_queue_get_event(event_queue);

    let accept_cbk = EventCallback(NonZeroUsize::new(1).unwrap());
    let mut callbacks = 2;
    event_queue_listen(
        event_queue,
        service_accept,
        accept_cbk,
        kernel_userspace::event::KernelEventQueueListenMode::OnLevelHigh,
    );

    loop {
        receive_event(
            event_queue_ev,
            kernel_userspace::event::ReceiveMode::LevelHigh,
        );
        while let Some(callback) = event_queue_pop(event_queue) {
            if callback == accept_cbk {
                let Some(sock) = socket_accept(service) else {
                    continue;
                };
                let event = socket_handle_get_event(sock, SocketEvents::RecvBufferEmpty);
                let cbk = EventCallback(NonZeroUsize::new(callbacks).unwrap());
                callbacks += 1;
                let queued = event_queue_listen(
                    event_queue,
                    event,
                    cbk,
                    kernel_userspace::event::KernelEventQueueListenMode::OnLevelLow,
                );
                connections.insert(
                    cbk,
                    GopMonitorInfo {
                        event: KernelReference::from_id(event),
                        sock: KernelReference::from_id(sock),
                        queued,
                    },
                );
            } else {
                let conn = connections.get(&callback).unwrap();
                match socket_recv(conn.sock.id()) {
                    Ok((msg, ty)) => {
                        if ty == KernelObjectType::Message {
                            let msg = MessageHandle::from_kref(KernelReference::from_id(msg));
                            let msg = msg.read_vec();
                            if let Ok(s) = core::str::from_utf8(&msg) {
                                print!("{s}")
                            } else {
                                println!("GOP STDOUT invalid bytes");
                            }
                            continue;
                        }
                        println!("GOP STDOUT only accepts messages")
                    }
                    Err(SocketRecieveResult::None) => continue,
                    Err(SocketRecieveResult::EOF) => (),
                }
                // did something to exit
                let conn = connections.remove(&callback).unwrap();
                event_queue_unlisten(event_queue, conn.queued);
            }
        }
    }
}

pub fn gop_entry() {
    // TODO: Once isnt mapped for everyone, map it
    spawn_thread(monitor_cursor_task);
    monitor_stdout_task();
}
