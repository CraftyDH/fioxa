use self::virtual_code::VirtualKeyCode;

pub mod virtual_code;
pub mod us_keyboard;

pub enum KeyboardEvent {
    Down(VirtualKeyCode),
    Up(VirtualKeyCode),
}