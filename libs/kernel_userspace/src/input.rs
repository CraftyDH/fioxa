use input::{keyboard::KeyboardEvent, mouse::MousePacket};

#[derive(Debug, Clone, Copy)]
pub enum InputServiceMessage {
    KeyboardEvent(KeyboardEvent),
    MouseEvent(MousePacket),
}
