use input::keyboard::KeyboardEvent;
use userspace::log::warn;

use super::Scancode;

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

impl Scancode for ScancodeSet2 {
    fn add_byte(&mut self, code: u8) -> Option<KeyboardEvent> {
        // If byte is an ack ignore it
        if code == 0xFA {
            return None;
        }
        match self.state {
            State::Start => match code {
                RELEASE_CODE => {
                    self.state = State::Release;
                    None
                }
                EXTENDED_CODE => {
                    self.state = State::Extended;
                    None
                }
                _ => {
                    if let Some(key) = self.map_scancode(code) {
                        return Some(KeyboardEvent::Down(key));
                    }
                    None
                }
            },

            State::Release => {
                self.state = State::Start;
                if let Some(key) = self.map_scancode(code) {
                    return Some(KeyboardEvent::Up(key));
                }
                None
            }
            State::Extended => match code {
                RELEASE_CODE => {
                    self.state = State::ExtendedRelease;
                    None
                }
                _ => {
                    self.state = State::Start;
                    if let Some(key) = self.map_extened_scancode(code) {
                        return Some(KeyboardEvent::Down(key));
                    }
                    None
                }
            },

            State::ExtendedRelease => {
                self.state = State::Start;
                if let Some(key) = self.map_extened_scancode(code) {
                    return Some(KeyboardEvent::Up(key));
                }
                None
            }
        }
    }
}

impl Default for ScancodeSet2 {
    fn default() -> Self {
        Self::new()
    }
}

impl ScancodeSet2 {
    pub const fn new() -> Self {
        Self {
            state: State::Start,
        }
    }

    fn map_scancode(&self, code: u8) -> Option<input::keyboard::virtual_code::VirtualKeyCode> {
        // Weird order
        // From https://wiki.osdev.org/PS/2_Keyboard
        use input::keyboard::virtual_code::*;
        Some(match code {
            0x01 => Function::F9.into(),
            0x03 => Function::F5.into(),
            0x04 => Function::F3.into(),
            0x05 => Function::F1.into(),
            0x06 => Function::F2.into(),
            0x07 => Function::F12.into(),
            0x09 => Function::F10.into(),
            0x0A => Function::F8.into(),
            0x0B => Function::F6.into(),
            0x0C => Function::F4.into(),
            0x0D => Control::Tab.into(),
            0x0E => Misc::BackTick.into(),
            0x11 => Modifier::LeftAlt.into(),
            0x12 => Modifier::LeftShift.into(),
            0x14 => Modifier::LeftControl.into(),
            0x15 => Letter::Q.into(),
            0x16 => Number::N1.into(),
            0x1A => Letter::Z.into(),
            0x1B => Letter::S.into(),
            0x1C => Letter::A.into(),
            0x1D => Letter::W.into(),
            0x1E => Number::N2.into(),
            0x21 => Letter::C.into(),
            0x22 => Letter::X.into(),
            0x23 => Letter::D.into(),
            0x24 => Letter::E.into(),
            0x25 => Number::N4.into(),
            0x26 => Number::N3.into(),
            0x29 => Control::Space.into(),
            0x2A => Letter::V.into(),
            0x2B => Letter::F.into(),
            0x2C => Letter::T.into(),
            0x2D => Letter::R.into(),
            0x2E => Number::N5.into(),
            0x31 => Letter::N.into(),
            0x32 => Letter::B.into(),
            0x33 => Letter::H.into(),
            0x34 => Letter::G.into(),
            0x35 => Letter::Y.into(),
            0x36 => Number::N6.into(),
            0x3A => Letter::M.into(),
            0x3B => Letter::J.into(),
            0x3C => Letter::U.into(),
            0x3D => Number::N7.into(),
            0x3E => Number::N8.into(),
            0x41 => Misc::Comma.into(),
            0x42 => Letter::K.into(),
            0x43 => Letter::I.into(),
            0x44 => Letter::O.into(),
            0x45 => Number::N0.into(),
            0x46 => Number::N9.into(),
            0x49 => Misc::Period.into(),
            0x4A => Misc::ForwardSlash.into(),
            0x4B => Letter::L.into(),
            0x4C => Misc::SemiColon.into(),
            0x4D => Letter::P.into(),
            0x4E => Misc::Hyphen.into(),
            0x52 => Misc::Quote.into(),
            0x54 => Misc::LeftBracket.into(),
            0x55 => Misc::Equals.into(),
            0x58 => Modifier::CapsLock.into(),
            0x59 => Modifier::RightShift.into(),
            0x5A => Control::Enter.into(),
            0x5B => Misc::RightBracket.into(),
            0x5D => Misc::BackSlash.into(),

            0x66 => Control::Backspace.into(),
            0x69 => Numpad::N1.into(),
            0x6B => Numpad::N4.into(),
            0x6C => Numpad::N7.into(),
            0x70 => Numpad::N0.into(),
            0x71 => Numpad::Period.into(),
            0x72 => Numpad::N2.into(),
            0x73 => Numpad::N5.into(),
            0x74 => Numpad::N6.into(),
            0x75 => Numpad::N8.into(),
            0x76 => Control::Escape.into(),
            0x77 => Modifier::NumLock.into(),
            0x78 => Function::F11.into(),
            0x79 => Numpad::Add.into(),
            0x7A => Numpad::N3.into(),
            0x7B => Numpad::Sub.into(),
            0x7C => Numpad::Mul.into(),
            0x7D => Numpad::N9.into(),
            0x7E => Modifier::ScrollLock.into(),

            0x83 => Function::F7.into(),

            0xE1 => Control::PauseBreak.into(),

            _ => {
                warn!("Unknown keycode Entered '{code:#X}'");
                return None;
            }
        })
    }

    fn map_extened_scancode(
        &self,
        code: u8,
    ) -> Option<input::keyboard::virtual_code::VirtualKeyCode> {
        // Weird order
        // From https://wiki.osdev.org/PS/2_Keyboard

        use input::keyboard::virtual_code::*;
        Some(match code {
            0x11 => Modifier::RightAlt.into(),
            0x14 => Modifier::RightControl.into(),
            0x1F => Modifier::LeftWindows.into(),
            0x27 => Modifier::RightWindows.into(),
            0x2F => Misc::MenuKey.into(),
            0x4A => Numpad::Div.into(),
            0x5A => Numpad::Enter.into(),
            0x69 => Control::End.into(),
            0x6B => Control::ArrowLeft.into(),
            0x6C => Control::Home.into(),
            0x70 => Control::Insert.into(),
            0x71 => Control::Delete.into(),
            0x72 => Control::ArrowDown.into(),
            0x74 => Control::ArrowRight.into(),
            0x75 => Control::ArrowUp.into(),
            0x7A => Control::PageDown.into(),
            0x7D => Control::PageUp.into(),

            _ => {
                warn!("Unknown ext keycode entered: {code:#X}");
                return None;
            }
        })
    }
}
