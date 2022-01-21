use alloc::fmt;

use super::scancode::keys::RawKeyCode;

#[derive(Debug)]
pub enum KeyCode {
    Unicode(char),
    SpecialCodes(SpecialCodes),
}

#[derive(Debug)]
pub enum SpecialCodes {
    Insert,
    Home,
    PageUp,
    PageDown,
    Delete,
    End,

    UpArrow,
    LeftArrow,
    RightArrow,
    DownArrow,

    Enter,
    Backspace,
}

impl fmt::Display for SpecialCodes {
    fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
        let pretty_print = match self {
            Self::Insert => "Insert",
            Self::Home => "Home",
            Self::PageUp => "PageUp",
            Self::PageDown => "PageDown",
            Self::Delete => "\u{7F}",
            Self::End => "End",

            Self::UpArrow => "\u{2191}",
            Self::DownArrow => "\u{2193}",
            Self::LeftArrow => "\u{2190}",
            Self::RightArrow => "\u{2192}",

            Self::Enter => "\n",
            Self::Backspace => "\x08",
        };
        write!(f, "{}", pretty_print)
    }
}

pub fn translate_raw_keycode(code: RawKeyCode, shift: bool, caps: bool, num_lock: bool) -> KeyCode {
    let normal_shift = if caps { !shift } else { shift };
    match code {
        // Numbers
        RawKeyCode::Num1 => KeyCode::Unicode(if shift { '!' } else { '1' }),
        RawKeyCode::Num2 => KeyCode::Unicode(if shift { '@' } else { '2' }),
        RawKeyCode::Num3 => KeyCode::Unicode(if shift { '#' } else { '3' }),
        RawKeyCode::Num4 => KeyCode::Unicode(if shift { '$' } else { '4' }),
        RawKeyCode::Num5 => KeyCode::Unicode(if shift { '%' } else { '5' }),
        RawKeyCode::Num6 => KeyCode::Unicode(if shift { '^' } else { '6' }),
        RawKeyCode::Num7 => KeyCode::Unicode(if shift { '&' } else { '7' }),
        RawKeyCode::Num8 => KeyCode::Unicode(if shift { '*' } else { '8' }),
        RawKeyCode::Num9 => KeyCode::Unicode(if shift { '(' } else { '9' }),
        RawKeyCode::Num0 => KeyCode::Unicode(if shift { ')' } else { '0' }),

        // Numpad
        RawKeyCode::Numpad0 => {
            if num_lock {
                KeyCode::Unicode('0')
            } else {
                KeyCode::SpecialCodes(SpecialCodes::Insert)
            }
        }
        RawKeyCode::Numpad1 => {
            if num_lock {
                KeyCode::Unicode('1')
            } else {
                KeyCode::SpecialCodes(SpecialCodes::End)
            }
        }
        RawKeyCode::Numpad2 => {
            if num_lock {
                KeyCode::Unicode('2')
            } else {
                KeyCode::SpecialCodes(SpecialCodes::DownArrow)
            }
        }
        RawKeyCode::Numpad3 => {
            if num_lock {
                KeyCode::Unicode('3')
            } else {
                KeyCode::SpecialCodes(SpecialCodes::PageDown)
            }
        }
        RawKeyCode::Numpad4 => {
            if num_lock {
                KeyCode::Unicode('4')
            } else {
                KeyCode::SpecialCodes(SpecialCodes::LeftArrow)
            }
        }
        RawKeyCode::Numpad5 => KeyCode::Unicode('5'),
        RawKeyCode::Numpad6 => {
            if num_lock {
                KeyCode::Unicode('6')
            } else {
                KeyCode::SpecialCodes(SpecialCodes::RightArrow)
            }
        }
        RawKeyCode::Numpad7 => {
            if num_lock {
                KeyCode::Unicode('7')
            } else {
                KeyCode::SpecialCodes(SpecialCodes::Home)
            }
        }
        RawKeyCode::Numpad8 => {
            if num_lock {
                KeyCode::Unicode('8')
            } else {
                KeyCode::SpecialCodes(SpecialCodes::UpArrow)
            }
        }
        RawKeyCode::Numpad9 => {
            if num_lock {
                KeyCode::Unicode('9')
            } else {
                KeyCode::SpecialCodes(SpecialCodes::PageUp)
            }
        }

        // Letters
        RawKeyCode::A => KeyCode::Unicode(if normal_shift { 'A' } else { 'a' }),
        RawKeyCode::B => KeyCode::Unicode(if normal_shift { 'B' } else { 'b' }),
        RawKeyCode::C => KeyCode::Unicode(if normal_shift { 'C' } else { 'c' }),
        RawKeyCode::D => KeyCode::Unicode(if normal_shift { 'D' } else { 'd' }),
        RawKeyCode::E => KeyCode::Unicode(if normal_shift { 'E' } else { 'e' }),
        RawKeyCode::F => KeyCode::Unicode(if normal_shift { 'F' } else { 'f' }),
        RawKeyCode::G => KeyCode::Unicode(if normal_shift { 'G' } else { 'g' }),
        RawKeyCode::H => KeyCode::Unicode(if normal_shift { 'H' } else { 'h' }),
        RawKeyCode::I => KeyCode::Unicode(if normal_shift { 'I' } else { 'i' }),
        RawKeyCode::J => KeyCode::Unicode(if normal_shift { 'J' } else { 'j' }),
        RawKeyCode::K => KeyCode::Unicode(if normal_shift { 'K' } else { 'k' }),
        RawKeyCode::L => KeyCode::Unicode(if normal_shift { 'L' } else { 'l' }),
        RawKeyCode::M => KeyCode::Unicode(if normal_shift { 'M' } else { 'm' }),
        RawKeyCode::N => KeyCode::Unicode(if normal_shift { 'N' } else { 'n' }),
        RawKeyCode::O => KeyCode::Unicode(if normal_shift { 'O' } else { 'o' }),
        RawKeyCode::P => KeyCode::Unicode(if normal_shift { 'P' } else { 'p' }),
        RawKeyCode::Q => KeyCode::Unicode(if normal_shift { 'Q' } else { 'q' }),
        RawKeyCode::R => KeyCode::Unicode(if normal_shift { 'R' } else { 'r' }),
        RawKeyCode::S => KeyCode::Unicode(if normal_shift { 'S' } else { 's' }),
        RawKeyCode::T => KeyCode::Unicode(if normal_shift { 'T' } else { 't' }),
        RawKeyCode::U => KeyCode::Unicode(if normal_shift { 'U' } else { 'u' }),
        RawKeyCode::V => KeyCode::Unicode(if normal_shift { 'V' } else { 'v' }),
        RawKeyCode::W => KeyCode::Unicode(if normal_shift { 'W' } else { 'w' }),
        RawKeyCode::X => KeyCode::Unicode(if normal_shift { 'X' } else { 'x' }),
        RawKeyCode::Y => KeyCode::Unicode(if normal_shift { 'Y' } else { 'y' }),
        RawKeyCode::Z => KeyCode::Unicode(if normal_shift { 'Z' } else { 'z' }),

        // Whitespace
        RawKeyCode::Space => KeyCode::Unicode(' '),
        // TODO: FIX for actual tab
        RawKeyCode::Tab => KeyCode::Unicode(' '),
        RawKeyCode::Enter => KeyCode::SpecialCodes(SpecialCodes::Enter),
        RawKeyCode::Backspace => KeyCode::SpecialCodes(SpecialCodes::Backspace),

        // Main Keyboard chars
        RawKeyCode::BackTick => KeyCode::Unicode(if normal_shift { '~' } else { '`' }),
        RawKeyCode::Hyphen => KeyCode::Unicode(if normal_shift { '_' } else { '-' }),
        RawKeyCode::Equals => KeyCode::Unicode(if normal_shift { '+' } else { '=' }),
        RawKeyCode::LeftBracket => KeyCode::Unicode(if normal_shift { '{' } else { ']' }),
        RawKeyCode::RightBracket => KeyCode::Unicode(if normal_shift { '{' } else { ']' }),
        RawKeyCode::BackSlash => KeyCode::Unicode(if normal_shift { '|' } else { '\\' }),
        RawKeyCode::SemiColon => KeyCode::Unicode(if normal_shift { ':' } else { ';' }),
        RawKeyCode::Quote => KeyCode::Unicode(if normal_shift { '"' } else { '\'' }),
        RawKeyCode::Comma => KeyCode::Unicode(if normal_shift { '<' } else { ',' }),
        RawKeyCode::Period => KeyCode::Unicode(if normal_shift { '>' } else { '.' }),
        RawKeyCode::Slash => KeyCode::Unicode(if normal_shift { '?' } else { '/' }),

        // Numpad chars
        RawKeyCode::NumpadSlash => KeyCode::Unicode('/'),
        RawKeyCode::NumpadMul => KeyCode::Unicode('*'),
        RawKeyCode::NumpadMinus => KeyCode::Unicode('-'),
        RawKeyCode::NumpadPlus => KeyCode::Unicode('+'),
        RawKeyCode::NumpadEnter => KeyCode::SpecialCodes(SpecialCodes::Enter),
        RawKeyCode::NumpadPeriod => {
            if num_lock {
                KeyCode::Unicode('.')
            } else {
                KeyCode::SpecialCodes(SpecialCodes::Delete)
            }
        }
        // Arrows
        RawKeyCode::UpArrow => KeyCode::Unicode('\u{2191}'),
        RawKeyCode::DownArrow => KeyCode::Unicode('\u{2193}'),
        RawKeyCode::LeftArrow => KeyCode::Unicode('\u{2190}'),
        RawKeyCode::RightArrow => KeyCode::Unicode('\u{2192}'),

        _ => KeyCode::Unicode('\u{0}'),
    }
}
