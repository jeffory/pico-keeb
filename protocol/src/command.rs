pub const MOD_LCTRL: u8 = 0x01;
pub const MOD_LSHIFT: u8 = 0x02;
pub const MOD_LALT: u8 = 0x04;
pub const MOD_LGUI: u8 = 0x08;
pub const MOD_RCTRL: u8 = 0x10;
pub const MOD_RSHIFT: u8 = 0x20;
pub const MOD_RALT: u8 = 0x40;
pub const MOD_RGUI: u8 = 0x80;

#[derive(Debug, Clone, Copy, Default, PartialEq, Eq)]
pub struct KeyChord {
    pub modifiers: u8,
    pub key: u8,
}

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum MouseButton {
    Left,
    Right,
    Middle,
}

impl MouseButton {
    pub const fn mask(self) -> u8 {
        match self {
            MouseButton::Left => 0x01,
            MouseButton::Right => 0x02,
            MouseButton::Middle => 0x04,
        }
    }
}

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum MediaKey {
    Play,
    Pause,
    Next,
    Prev,
    VolUp,
    VolDn,
    Mute,
}

impl MediaKey {
    // Consumer Control usage codes (HID Usage Page 0x0C).
    pub const fn usage(self) -> u16 {
        match self {
            MediaKey::Play => 0x00B0,
            MediaKey::Pause => 0x00B1,
            MediaKey::Next => 0x00B5,
            MediaKey::Prev => 0x00B6,
            MediaKey::VolUp => 0x00E9,
            MediaKey::VolDn => 0x00EA,
            MediaKey::Mute => 0x00E2,
        }
    }
}

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum Command<'a> {
    Type(&'a str),
    Key(KeyChord),
    Hold(KeyChord),
    Release(KeyChord),
    Move { dx: i16, dy: i16 },
    Click(MouseButton),
    Scroll(i8),
    Media(MediaKey),
    Delay(u32),
    Reset,
}
