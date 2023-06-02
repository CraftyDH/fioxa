use crossbeam_queue::ArrayQueue;
use kernel_userspace::{stream::StreamMessage, syscall::yield_now};
use lazy_static::lazy_static;
use x86_64::instructions::interrupts::without_interrupts;

use input::mouse::MousePacket;

use super::gop::{Pos, WRITER};

const MOUSE_POINTER: &[u16; 16] = &[
    0b1111111111000000,
    0b1111111110000000,
    0b1111111100000000,
    0b1111111000000000,
    0b1111110000000000,
    0b1111100000000000,
    0b1111000000000000,
    0b1110000000000000,
    0b1100000000000000,
    0b1000000000000000,
    0b0000000000000000,
    0b0000000000000000,
    0b0000000000000000,
    0b0000000000000000,
    0b0000000000000000,
    0b0000000000000000,
];

pub fn print_cursor(pos: &mut Pos, msg: StreamMessage) {
    let mouse: MousePacket = msg.read_data();

    let mut colour: u32 = 0x50_50_50;

    if mouse.left {
        colour |= 0xFF_00_00;
    }

    if mouse.right {
        colour |= 0x00_FF_00;
    }

    if mouse.middle {
        colour |= 0x00_00_FF;
    }

    pos.x = pos.x.saturating_add_signed(mouse.x_mov as isize);
    pos.y = pos.y.saturating_add_signed(mouse.y_mov as isize);

    without_interrupts(|| {
        let gop_mutex = &mut WRITER.lock();
        let gop_info = &gop_mutex.gop;

        if pos.x > gop_info.horizonal - 8 {
            pos.x = gop_info.horizonal - 8
        }

        if pos.y > gop_info.vertical - 16 {
            pos.y = gop_info.vertical - 16
        }
        gop_mutex.draw_cursor(*pos, colour, MOUSE_POINTER);
    });
}
