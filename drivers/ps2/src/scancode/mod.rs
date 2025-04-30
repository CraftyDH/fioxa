use input::keyboard::KeyboardEvent;

pub mod keys;
pub mod set2;

pub trait Scancode: Send + Sync {
    fn add_byte(&mut self, code: u8) -> Option<KeyboardEvent>;
}
