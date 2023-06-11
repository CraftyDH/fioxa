use serde::{Deserialize, Serialize};

use input::{keyboard::KeyboardEvent, mouse::MousePacket};

#[derive(Debug, Clone, Copy, Serialize, Deserialize)]
pub enum InputServiceMessage {
    KeyboardEvent(KeyboardEvent),
    MouseEvent(MousePacket),
}
