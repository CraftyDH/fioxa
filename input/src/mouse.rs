use serde::{Deserialize, Serialize};

#[derive(Debug, Clone, Copy, Serialize, Deserialize)]
pub struct MousePacket {
    pub left: bool,
    pub right: bool,
    pub middle: bool,
    pub x_mov: i8,
    pub y_mov: i8,
}
