#[derive(Debug, Clone, Copy)]
pub enum RawKeyCodeState {
    Up(RawKeyCode),
    Down(RawKeyCode),
}

#[derive(Debug, Clone, Copy)]
pub enum RawKeyCode {
    // Control Characters
    Escape,
    CapsLock,
    Backspace,
    Enter,
    LeftShift,
    RightShift,
    LeftControl,
    RightControl,
    LeftWindows,
    RightWindows,
    LeftAlt,
    RightAlt,
    MenuKey,
    NumLock,

    // Useless middle panel control characters
    // TODO: Better name
    ScrollLock,
    Insert,
    Home,
    PageUp,
    Delete,
    End,
    PageDown,

    // Virtual Characters
    // AKA Magic combo
    PauseBreak,
    // PrintScreen,

    // Function keys
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

    // Numbers
    Num1,
    Num2,
    Num3,
    Num4,
    Num5,
    Num6,
    Num7,
    Num8,
    Num9,
    Num0,

    // Letters
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

    // Other characters
    BackTick,
    Hyphen,
    Equals,
    LeftBracket,
    RightBracket,
    BackSlash,
    SemiColon,
    Quote,
    Comma,
    Period,
    Slash,

    // Whitespace Characters
    Tab,
    Space,

    // Numpad Numbers
    Numpad1,
    Numpad2,
    Numpad3,
    Numpad4,
    Numpad5,
    Numpad6,
    Numpad7,
    Numpad8,
    Numpad9,
    Numpad0,

    // Numpad Other
    NumpadSlash,
    NumpadMul,
    NumpadMinus,
    NumpadPlus,
    NumpadEnter,
    NumpadPeriod,

    // Arrows
    UpArrow,
    LeftArrow,
    DownArrow,
    RightArrow,
}
