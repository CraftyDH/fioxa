use alloc::vec::Vec;
use kernel_userspace::{
    input::InputServiceMessage,
    service::get_public_service_id,
    syscall::{service_subscribe, yield_now},
};

use input::mouse::MousePacket;

use crate::scheduling::without_context_switch;

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

pub fn monitor_cursor_task() {
    let mut buffer = Vec::new();
    let mouse_id;
    // Poll the mouse until the service exists
    loop {
        if let Some(m) = get_public_service_id("INPUT:MOUSE", &mut buffer) {
            mouse_id = m;
            break;
        }
        yield_now();
    }
    service_subscribe(mouse_id);

    let mut mouse_pos: Pos = Pos { x: 0, y: 0 };

    loop {
        let msg =
            kernel_userspace::syscall::receive_service_message_blocking(mouse_id, &mut buffer)
                .unwrap();

        match msg.message {
            InputServiceMessage::MouseEvent(packet) => print_cursor(&mut mouse_pos, packet),
            _ => println!("Mouse got non mouse packet"),
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

    without_context_switch(|| {
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
