use alloc::collections::BTreeMap;
use alloc::string::String;
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

pub static WRITER: OnceCell<Mutex<Writer>> = OnceCell::uninit();

#[macro_export]
macro_rules! early_println {
    () => (print!("\n"));
    ($($arg:tt)*) => (early_print!("{}\n", format_args!($($arg)*)));
}

#[macro_export]
macro_rules! early_print {
    ($($arg:tt)*) => ($crate::screen::gop::_print(format_args!($($arg)*)));
}

#[macro_export]
macro_rules! colour {
    ($colour:expr) => {
        $crate::gop::WRITER.lock().set_colour($colour)
    };
}

use crate::scheduling::with_held_interrupts;
use crate::terminal::{Cell, Writer};
use core::fmt::Arguments;
use spin::mutex::Mutex;

use super::mouse::monitor_cursor_task;
use super::psf1::PSF1Font;

#[doc(hidden)]
pub fn _print(args: Arguments) {
    loop {
        // Prevent task from being scheduled away with mutex
        let res = with_held_interrupts(|| {
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
                            let s = String::from_utf8_lossy(&msg);
                            with_held_interrupts(|| {
                                let mut w = WRITER.get().unwrap().lock();
                                w.write_str(&s).unwrap();
                            });
                            continue;
                        }
                        error!("GOP STDOUT only accepts messages")
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
