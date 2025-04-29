#[derive(Debug, Clone, Copy)]
pub enum VirtualKeyCode {
    Modifier(Modifier),
    Control(Control),
    Number(Number),
    Numpad(Numpad),
    Letter(Letter),
    Misc(Misc),
    Function(Function),
}

impl From<Modifier> for VirtualKeyCode {
    fn from(value: Modifier) -> Self {
        Self::Modifier(value)
    }
}

impl From<Control> for VirtualKeyCode {
    fn from(value: Control) -> Self {
        Self::Control(value)
    }
}

impl From<Number> for VirtualKeyCode {
    fn from(value: Number) -> Self {
        Self::Number(value)
    }
}

impl From<Numpad> for VirtualKeyCode {
    fn from(value: Numpad) -> Self {
        Self::Numpad(value)
    }
}

impl From<Letter> for VirtualKeyCode {
    fn from(value: Letter) -> Self {
        Self::Letter(value)
    }
}

impl From<Misc> for VirtualKeyCode {
    fn from(value: Misc) -> Self {
        Self::Misc(value)
    }
}

impl From<Function> for VirtualKeyCode {
    fn from(value: Function) -> Self {
        Self::Function(value)
    }
}

#[derive(Debug, Clone, Copy)]
pub enum Modifier {
    LeftShift,
    RightShift,
    LeftAlt,
    RightAlt,
    LeftWindows,
    RightWindows,
    LeftControl,
    RightControl,
    CapsLock,
    ScrollLock,
    NumLock,
}

#[derive(Debug, Clone, Copy)]
pub enum Control {
    Escape,
    Enter,
    Space,
    Backspace,
    Delete,
    Tab,
    Insert,
    Home,
    End,
    PageUp,
    PageDown,
    ArrowLeft,
    ArrowRight,
    ArrowUp,
    ArrowDown,
    PrintScreen,
    PauseBreak,
}

#[derive(Debug, Clone, Copy)]
pub enum Number {
    N0,
    N1,
    N2,
    N3,
    N4,
    N5,
    N6,
    N7,
    N8,
    N9,
}

#[derive(Debug, Clone, Copy)]
pub enum Numpad {
    N0,
    N1,
    N2,
    N3,
    N4,
    N5,
    N6,
    N7,
    N8,
    N9,
    Div,
    Mul,
    Sub,
    Add,
    Enter,
    Period,
}

#[derive(Debug, Clone, Copy)]
pub enum Letter {
    A,
    B,
    C,
    D,
    E,
    F,
    G,
    H,
    I,
    J,
    K,
    L,
    M,
    N,
    O,
    P,
    Q,
    R,
    S,
    T,
    U,
    V,
    W,
    X,
    Y,
    Z,
}

#[derive(Debug, Clone, Copy)]
pub enum Misc {
    Hyphen,
    Equals,
    Comma,
    Period,
    SemiColon,
    ForwardSlash,
    BackSlash,
    BackTick,
    LeftBracket,
    RightBracket,
    Quote,
    MenuKey,
}

#[derive(Debug, Clone, Copy)]
pub enum Function {
    F0,
    F1,
    F2,
    F3,
    F4,
    F5,
    F6,
    F7,
    F8,
    F9,
    F10,
    F11,
    F12,
    F13,
    F14,
    F15,
    F16,
    F17,
    F18,
    F19,
    F20,
    F21,
    F22,
    F23,
    F24,
}
