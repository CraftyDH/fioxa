use serde::{Deserialize, Serialize};

use self::virtual_code::VirtualKeyCode;

pub mod us_keyboard;
pub mod virtual_code;

#[derive(Debug, Clone, Copy, Serialize, Deserialize)]
pub enum KeyboardEvent {
    Down(VirtualKeyCode),
    Up(VirtualKeyCode),
}
