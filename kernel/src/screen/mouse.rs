use kernel_userspace::{
    backoff_sleep,
    event::{receive_event, ReceiveMode},
    input::InputServiceMessage,
    message::MessageHandle,
    object::{KernelObjectType, KernelReference},
    service::deserialize,
    socket::{
        socket_connect, socket_handle_get_event, socket_recv, SocketEvents, SocketRecieveResult,
    },
};

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
    // loop to in theory allow recovery if the connection dies
    'outer: loop {
        // Poll the mouse until the service exists
        let mouse_id = KernelReference::from_id(backoff_sleep(|| socket_connect("INPUT:MOUSE")));
        let mouse_event = KernelReference::from_id(socket_handle_get_event(
            mouse_id.id(),
            SocketEvents::RecvBufferEmpty,
        ));

        let mut mouse_pos: Pos = Pos { x: 0, y: 0 };

        loop {
            receive_event(mouse_event.id(), ReceiveMode::LevelLow);
            match socket_recv(mouse_id.id()) {
                Ok((msg, ty)) => {
                    if ty != KernelObjectType::Message {
                        error!("Cursor task got non message");
                        continue 'outer;
                    }
                    let msg = MessageHandle::from_kref(KernelReference::from_id(msg));
                    let data = msg.read_vec();
                    match deserialize(&data).unwrap() {
                        InputServiceMessage::MouseEvent(packet) => {
                            print_cursor(&mut mouse_pos, packet)
                        }
                        _ => {
                            error!("Mouse got non mouse packet");
                            continue 'outer;
                        }
                    }
                }
                Err(SocketRecieveResult::None) => (),
                Err(SocketRecieveResult::EOF) => continue 'outer,
            }
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
