use super::keys::{RawKeyCode, RawKeyCodeState};

const RELEASE_CODE: u8 = 0xF0;
const EXTENDED_CODE: u8 = 0xE0;
enum State {
    Start,
    Release,
    Extended,
    ExtendedRelease,
}

pub struct ScancodeSet2 {
    state: State,
}

impl ScancodeSet2 {
    pub const fn new() -> Self {
        Self {
            state: State::Start,
        }
    }

    pub fn add_byte(&mut self, code: u8) -> Option<RawKeyCodeState> {
        // If byte is an ack ignore it
        if code == 0xFA {
            return None;
        }
        match self.state {
            State::Start => match code {
                RELEASE_CODE => {
                    self.state = State::Release;
                    return None;
                }
                EXTENDED_CODE => {
                    self.state = State::Extended;
                    return None;
                }
                _ => {
                    if let Some(key) = self.map_scancode(code) {
                        return Some(RawKeyCodeState::Down(key));
                    }
                    None
                }
            },

            State::Release => {
                self.state = State::Start;
                if let Some(key) = self.map_scancode(code) {
                    return Some(RawKeyCodeState::Up(key));
                }
                None
            }
            State::Extended => match code {
                RELEASE_CODE => {
                    self.state = State::ExtendedRelease;
                    return None;
                }
                _ => {
                    self.state = State::Start;
                    if let Some(key) = self.map_extened_scancode(code) {
                        return Some(RawKeyCodeState::Down(key));
                    }
                    None
                }
            },

            State::ExtendedRelease => {
                self.state = State::Start;
                if let Some(key) = self.map_extened_scancode(code) {
                    return Some(RawKeyCodeState::Up(key));
                }
                None
            }
        }
    }

    fn map_scancode(&self, code: u8) -> Option<RawKeyCode> {
        // Weird order
        // From https://wiki.osdev.org/PS/2_Keyboard
        match code {
            0x01 => Some(RawKeyCode::F9),
            0x03 => Some(RawKeyCode::F5),
            0x04 => Some(RawKeyCode::F3),
            0x05 => Some(RawKeyCode::F1),
            0x06 => Some(RawKeyCode::F2),
            0x07 => Some(RawKeyCode::F12),
            0x09 => Some(RawKeyCode::F10),
            0x0A => Some(RawKeyCode::F8),
            0x0B => Some(RawKeyCode::F6),
            0x0C => Some(RawKeyCode::F4),
            0x0D => Some(RawKeyCode::Tab),
            0x0E => Some(RawKeyCode::BackTick),
            0x11 => Some(RawKeyCode::LeftAlt),
            0x12 => Some(RawKeyCode::LeftShift),
            0x14 => Some(RawKeyCode::LeftControl),
            0x15 => Some(RawKeyCode::Q),
            0x16 => Some(RawKeyCode::Num1),
            0x1A => Some(RawKeyCode::Z),
            0x1B => Some(RawKeyCode::S),
            0x1C => Some(RawKeyCode::A),
            0x1D => Some(RawKeyCode::W),
            0x1E => Some(RawKeyCode::Num2),
            0x21 => Some(RawKeyCode::C),
            0x22 => Some(RawKeyCode::X),
            0x23 => Some(RawKeyCode::D),
            0x24 => Some(RawKeyCode::E),
            0x25 => Some(RawKeyCode::Num4),
            0x26 => Some(RawKeyCode::Num3),
            0x29 => Some(RawKeyCode::Space),
            0x2A => Some(RawKeyCode::V),
            0x2B => Some(RawKeyCode::F),
            0x2C => Some(RawKeyCode::T),
            0x2D => Some(RawKeyCode::R),
            0x2E => Some(RawKeyCode::Num5),
            0x31 => Some(RawKeyCode::N),
            0x32 => Some(RawKeyCode::B),
            0x33 => Some(RawKeyCode::H),
            0x34 => Some(RawKeyCode::G),
            0x35 => Some(RawKeyCode::Y),
            0x36 => Some(RawKeyCode::Num6),
            0x3A => Some(RawKeyCode::M),
            0x3B => Some(RawKeyCode::J),
            0x3C => Some(RawKeyCode::U),
            0x3D => Some(RawKeyCode::Num7),
            0x3E => Some(RawKeyCode::Num8),
            0x41 => Some(RawKeyCode::Comma),
            0x42 => Some(RawKeyCode::K),
            0x43 => Some(RawKeyCode::I),
            0x44 => Some(RawKeyCode::O),
            0x45 => Some(RawKeyCode::Num0),
            0x46 => Some(RawKeyCode::Num9),
            0x49 => Some(RawKeyCode::Period),
            0x4A => Some(RawKeyCode::Slash),
            0x4B => Some(RawKeyCode::L),
            0x4C => Some(RawKeyCode::SemiColon),
            0x4D => Some(RawKeyCode::P),
            0x4E => Some(RawKeyCode::Hyphen),
            0x52 => Some(RawKeyCode::Quote),
            0x54 => Some(RawKeyCode::LeftBracket),
            0x55 => Some(RawKeyCode::Equals),
            0x58 => Some(RawKeyCode::CapsLock),
            0x59 => Some(RawKeyCode::RightShift),
            0x5A => Some(RawKeyCode::Enter),
            0x5B => Some(RawKeyCode::RightBracket),
            0x5D => Some(RawKeyCode::BackSlash),

            0x66 => Some(RawKeyCode::Backspace),
            0x69 => Some(RawKeyCode::Numpad1),
            0x6B => Some(RawKeyCode::Numpad4),
            0x6C => Some(RawKeyCode::Numpad7),
            0x70 => Some(RawKeyCode::Numpad0),
            0x71 => Some(RawKeyCode::NumpadPeriod),
            0x72 => Some(RawKeyCode::Numpad2),
            0x73 => Some(RawKeyCode::Numpad5),
            0x74 => Some(RawKeyCode::Numpad6),
            0x75 => Some(RawKeyCode::Numpad8),
            0x76 => Some(RawKeyCode::Escape),
            0x77 => Some(RawKeyCode::NumLock),
            0x78 => Some(RawKeyCode::F11),
            0x79 => Some(RawKeyCode::NumpadPlus),
            0x7A => Some(RawKeyCode::Numpad3),
            0x7B => Some(RawKeyCode::NumpadMinus),
            0x7C => Some(RawKeyCode::NumpadMul),
            0x7D => Some(RawKeyCode::Numpad9),
            0x7E => Some(RawKeyCode::ScrollLock),

            0x83 => Some(RawKeyCode::F7),

            0xE1 => Some(RawKeyCode::PauseBreak),

            _ => {
                println!("Unknown keycode Entered '{:#X}'", code);
                None
            }
        }
    }

    fn map_extened_scancode(&self, code: u8) -> Option<RawKeyCode> {
        // Weird order
        // From https://wiki.osdev.org/PS/2_Keyboard

        match code {
            0x11 => Some(RawKeyCode::RightAlt),
            0x14 => Some(RawKeyCode::RightControl),
            0x1F => Some(RawKeyCode::LeftWindows),
            0x27 => Some(RawKeyCode::RightWindows),
            0x2F => Some(RawKeyCode::MenuKey),

            0x4A => Some(RawKeyCode::NumpadSlash),

            0x5A => Some(RawKeyCode::NumpadEnter),

            0x69 => Some(RawKeyCode::End),
            0x6B => Some(RawKeyCode::LeftArrow),
            0x6C => Some(RawKeyCode::Home),
            0x70 => Some(RawKeyCode::Insert),
            0x71 => Some(RawKeyCode::Delete),
            0x72 => Some(RawKeyCode::DownArrow),
            0x74 => Some(RawKeyCode::RightArrow),
            0x75 => Some(RawKeyCode::UpArrow),
            0x7A => Some(RawKeyCode::PageDown),
            0x7D => Some(RawKeyCode::PageUp),

            _ => {
                println!("Unknown ext keycode entered: {:#X}", code);
                None
            }
        }
    }
}
