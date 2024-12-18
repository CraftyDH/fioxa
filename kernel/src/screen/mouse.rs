use alloc::vec::Vec;
use kernel_userspace::{input::InputServiceMessage, service::SimpleService};

use input::mouse::MousePacket;

use crate::scheduling::with_held_interrupts;

use super::gop::{Pos, WRITER};

pub const MOUSE_POINTER: &[u16; 16] = &[
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

pub fn monitor_cursor_task() {
    let mut mouse = SimpleService::with_name("INPUT:MOUSE");

    let mut mouse_pos: Pos = Pos { x: 0, y: 0 };

    let mut handles = Vec::new();

    loop {
        let packet = mouse.recv_val(&mut handles).unwrap();

        match packet {
            InputServiceMessage::KeyboardEvent(_) => panic!(),
            InputServiceMessage::MouseEvent(mouse) => print_cursor(&mut mouse_pos, mouse),
        }
    }
}

pub fn print_cursor(pos: &mut Pos, mouse: MousePacket) {
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

    with_held_interrupts(|| {
        let gop_mutex = &mut WRITER.get().unwrap().lock();
        let gop_info = &gop_mutex.screen.gop;

        if pos.x > gop_info.horizonal - 8 {
            pos.x = gop_info.horizonal - 8
        }

        if pos.y > gop_info.vertical - 16 {
            pos.y = gop_info.vertical - 16
        }
        gop_mutex.update_cursor(*pos, colour);
    });
}
