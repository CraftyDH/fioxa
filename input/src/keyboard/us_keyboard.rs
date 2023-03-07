use super::virtual_code::{Control, Letter, Misc, Number, Numpad, VirtualKeyCode};

pub struct USKeymap;

impl USKeymap {
    pub fn get_unicode(
        keycode: VirtualKeyCode,
        lshift: bool,
        rshift: bool,
        caps: bool,
        numlock: bool,
    ) -> char {
        let shift = lshift | rshift;
        let caps_shift = shift ^ caps;
        match keycode {
            VirtualKeyCode::Number(number) => {
                if shift {
                    match number {
                        Number::N0 => ')',
                        Number::N1 => '!',
                        Number::N2 => '@',
                        Number::N3 => '#',
                        Number::N4 => '$',
                        Number::N5 => '%',
                        Number::N6 => '^',
                        Number::N7 => '&',
                        Number::N8 => '*',
                        Number::N9 => '(',
                    }
                } else {
                    match number {
                        Number::N0 => '0',
                        Number::N1 => '1',
                        Number::N2 => '2',
                        Number::N3 => '3',
                        Number::N4 => '4',
                        Number::N5 => '5',
                        Number::N6 => '6',
                        Number::N7 => '7',
                        Number::N8 => '8',
                        Number::N9 => '9',
                    }
                }
            }
            VirtualKeyCode::Numpad(key) => {
                if numlock {
                    match key {
                        Numpad::N0 => '0',
                        Numpad::N1 => '1',
                        Numpad::N2 => '2',
                        Numpad::N3 => '3',
                        Numpad::N4 => '4',
                        Numpad::N5 => '5',
                        Numpad::N6 => '6',
                        Numpad::N7 => '7',
                        Numpad::N8 => '8',
                        Numpad::N9 => '9',
                        Numpad::Enter => '\n',
                        Numpad::Period => '.',
                        _ => '\0',
                    }
                } else {
                    match key {
                        Numpad::N5 => '5',
                        Numpad::Enter => '\n',
                        Numpad::Period => '.',
                        _ => '\0',
                    }
                }
            }
            VirtualKeyCode::Letter(letter) => {
                if caps_shift {
                    match letter {
                        Letter::A => 'A',
                        Letter::B => 'B',
                        Letter::C => 'C',
                        Letter::D => 'D',
                        Letter::E => 'E',
                        Letter::F => 'F',
                        Letter::G => 'G',
                        Letter::H => 'H',
                        Letter::I => 'I',
                        Letter::J => 'J',
                        Letter::K => 'K',
                        Letter::L => 'L',
                        Letter::M => 'M',
                        Letter::N => 'N',
                        Letter::O => 'O',
                        Letter::P => 'P',
                        Letter::Q => 'Q',
                        Letter::R => 'R',
                        Letter::S => 'S',
                        Letter::T => 'T',
                        Letter::U => 'U',
                        Letter::V => 'V',
                        Letter::W => 'W',
                        Letter::X => 'X',
                        Letter::Y => 'Y',
                        Letter::Z => 'Z',
                    }
                } else {
                    match letter {
                        Letter::A => 'a',
                        Letter::B => 'b',
                        Letter::C => 'c',
                        Letter::D => 'd',
                        Letter::E => 'e',
                        Letter::F => 'f',
                        Letter::G => 'g',
                        Letter::H => 'h',
                        Letter::I => 'i',
                        Letter::J => 'j',
                        Letter::K => 'k',
                        Letter::L => 'l',
                        Letter::M => 'm',
                        Letter::N => 'n',
                        Letter::O => 'o',
                        Letter::P => 'p',
                        Letter::Q => 'q',
                        Letter::R => 'r',
                        Letter::S => 's',
                        Letter::T => 't',
                        Letter::U => 'u',
                        Letter::V => 'v',
                        Letter::W => 'w',
                        Letter::X => 'x',
                        Letter::Y => 'y',
                        Letter::Z => 'z',
                    }
                }
            }
            VirtualKeyCode::Control(key) => match key {
                Control::Enter => '\n',
                Control::Space => ' ',
                Control::Backspace => '\x08',
                Control::Delete => '\u{7F}',
                Control::Tab => '\x09',
                _ => '\0',
            },
            VirtualKeyCode::Misc(key) => {
                if shift {
                    match key {
                        Misc::Hyphen => '_',
                        Misc::Equals => '+',
                        Misc::Comma => '<',
                        Misc::Period => '>',
                        Misc::SemiColon => ':',
                        Misc::ForwardSlash => '?',
                        Misc::BackSlash => '|',
                        Misc::BackTick => '~',
                        Misc::LeftBracket => '{',
                        Misc::RightBracket => '}',
                        Misc::Quote => '"',
                        Misc::MenuKey => '\0',
                    }
                } else {
                    match key {
                        Misc::Hyphen => '-',
                        Misc::Equals => '=',
                        Misc::Comma => ',',
                        Misc::Period => '.',
                        Misc::SemiColon => ';',
                        Misc::ForwardSlash => '/',
                        Misc::BackSlash => '\\',
                        Misc::BackTick => '`',
                        Misc::LeftBracket => '[',
                        Misc::RightBracket => ']',
                        Misc::Quote => '\'',
                        Misc::MenuKey => '\0',
                    }
                }
            }
            VirtualKeyCode::Function(_) => '\0',
            VirtualKeyCode::Modifier(_) => '\0',
        }
    }
}
