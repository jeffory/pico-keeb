//! Name → HID code lookups.
//!
//! Shared so the host CLI can translate user input ("CTRL+ALT+DEL",
//! "ENTER", "VOLUP") into wire-protocol payload bytes. The firmware
//! itself has no need for these lookups.

use crate::command::{
    KeyChord, MediaKey, MouseButton, MOD_LALT, MOD_LCTRL, MOD_LGUI, MOD_LSHIFT, MOD_RALT, MOD_RCTRL,
    MOD_RGUI, MOD_RSHIFT,
};

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum NameError {
    Empty,
    InvalidKey,
    InvalidMouseButton,
    InvalidMedia,
}

impl NameError {
    pub const fn as_str(self) -> &'static str {
        match self {
            NameError::Empty => "empty",
            NameError::InvalidKey => "invalid key",
            NameError::InvalidMouseButton => "invalid mouse button",
            NameError::InvalidMedia => "invalid media key",
        }
    }
}

impl core::fmt::Display for NameError {
    fn fmt(&self, f: &mut core::fmt::Formatter<'_>) -> core::fmt::Result {
        f.write_str(self.as_str())
    }
}

pub fn parse_chord(s: &str) -> Result<KeyChord, NameError> {
    let s = s.trim();
    if s.is_empty() {
        return Err(NameError::Empty);
    }
    let mut chord = KeyChord::default();
    for token in s.split('+') {
        let t = token.trim();
        if t.is_empty() {
            return Err(NameError::InvalidKey);
        }
        if let Some(m) = modifier_from_name(t) {
            chord.modifiers |= m;
        } else if let Some(k) = key_from_name(t) {
            if chord.key != 0 {
                return Err(NameError::InvalidKey);
            }
            chord.key = k;
        } else {
            return Err(NameError::InvalidKey);
        }
    }
    Ok(chord)
}

pub fn mouse_button_from_name(s: &str) -> Result<MouseButton, NameError> {
    if s.eq_ignore_ascii_case("LEFT") || s.eq_ignore_ascii_case("L") {
        Ok(MouseButton::Left)
    } else if s.eq_ignore_ascii_case("RIGHT") || s.eq_ignore_ascii_case("R") {
        Ok(MouseButton::Right)
    } else if s.eq_ignore_ascii_case("MIDDLE") || s.eq_ignore_ascii_case("M") {
        Ok(MouseButton::Middle)
    } else {
        Err(NameError::InvalidMouseButton)
    }
}

pub fn media_from_name(s: &str) -> Result<MediaKey, NameError> {
    Ok(if s.eq_ignore_ascii_case("PLAY") {
        MediaKey::Play
    } else if s.eq_ignore_ascii_case("PAUSE") {
        MediaKey::Pause
    } else if s.eq_ignore_ascii_case("NEXT") {
        MediaKey::Next
    } else if s.eq_ignore_ascii_case("PREV") || s.eq_ignore_ascii_case("PREVIOUS") {
        MediaKey::Prev
    } else if s.eq_ignore_ascii_case("VOLUP") {
        MediaKey::VolUp
    } else if s.eq_ignore_ascii_case("VOLDN") || s.eq_ignore_ascii_case("VOLDOWN") {
        MediaKey::VolDn
    } else if s.eq_ignore_ascii_case("MUTE") {
        MediaKey::Mute
    } else {
        return Err(NameError::InvalidMedia);
    })
}

pub fn modifier_from_name(n: &str) -> Option<u8> {
    const TABLE: &[(&str, u8)] = &[
        ("CTRL", MOD_LCTRL),
        ("CONTROL", MOD_LCTRL),
        ("LCTRL", MOD_LCTRL),
        ("RCTRL", MOD_RCTRL),
        ("SHIFT", MOD_LSHIFT),
        ("LSHIFT", MOD_LSHIFT),
        ("RSHIFT", MOD_RSHIFT),
        ("ALT", MOD_LALT),
        ("LALT", MOD_LALT),
        ("RALT", MOD_RALT),
        ("ALTGR", MOD_RALT),
        ("GUI", MOD_LGUI),
        ("LGUI", MOD_LGUI),
        ("RGUI", MOD_RGUI),
        ("WIN", MOD_LGUI),
        ("CMD", MOD_LGUI),
        ("META", MOD_LGUI),
        ("SUPER", MOD_LGUI),
    ];
    for &(name, m) in TABLE {
        if n.eq_ignore_ascii_case(name) {
            return Some(m);
        }
    }
    None
}

pub fn key_from_name(n: &str) -> Option<u8> {
    if n.len() == 1 {
        let b = n.as_bytes()[0];
        if b.is_ascii_alphabetic() {
            let c = b.to_ascii_uppercase();
            return Some(0x04 + (c - b'A'));
        }
        if (b'1'..=b'9').contains(&b) {
            return Some(0x1E + (b - b'1'));
        }
        if b == b'0' {
            return Some(0x27);
        }
    }
    if let Some(rest) = strip_prefix_ignore_case(n, "F") {
        if let Ok(num) = rest.parse::<u8>() {
            if (1..=12).contains(&num) {
                return Some(0x3A + (num - 1));
            }
            if (13..=24).contains(&num) {
                return Some(0x68 + (num - 13));
            }
        }
    }
    const NAMES: &[(&str, u8)] = &[
        ("ENTER", 0x28),
        ("RETURN", 0x28),
        ("ESC", 0x29),
        ("ESCAPE", 0x29),
        ("BACKSPACE", 0x2A),
        ("BS", 0x2A),
        ("TAB", 0x2B),
        ("SPACE", 0x2C),
        ("MINUS", 0x2D),
        ("DASH", 0x2D),
        ("EQUAL", 0x2E),
        ("EQUALS", 0x2E),
        ("LBRACKET", 0x2F),
        ("RBRACKET", 0x30),
        ("BACKSLASH", 0x31),
        ("SEMICOLON", 0x33),
        ("QUOTE", 0x34),
        ("APOSTROPHE", 0x34),
        ("BACKTICK", 0x35),
        ("GRAVE", 0x35),
        ("COMMA", 0x36),
        ("PERIOD", 0x37),
        ("DOT", 0x37),
        ("SLASH", 0x38),
        ("CAPSLOCK", 0x39),
        ("PRINTSCREEN", 0x46),
        ("PRTSC", 0x46),
        ("SCROLLLOCK", 0x47),
        ("PAUSE", 0x48),
        ("INSERT", 0x49),
        ("INS", 0x49),
        ("HOME", 0x4A),
        ("PAGEUP", 0x4B),
        ("PGUP", 0x4B),
        ("DELETE", 0x4C),
        ("DEL", 0x4C),
        ("END", 0x4D),
        ("PAGEDOWN", 0x4E),
        ("PGDN", 0x4E),
        ("RIGHT", 0x4F),
        ("ARROWRIGHT", 0x4F),
        ("LEFT", 0x50),
        ("ARROWLEFT", 0x50),
        ("DOWN", 0x51),
        ("ARROWDOWN", 0x51),
        ("UP", 0x52),
        ("ARROWUP", 0x52),
        ("NUMLOCK", 0x53),
        ("MENU", 0x65),
        ("APPS", 0x65),
    ];
    for &(name, usage) in NAMES {
        if n.eq_ignore_ascii_case(name) {
            return Some(usage);
        }
    }
    None
}

fn strip_prefix_ignore_case<'a>(s: &'a str, prefix: &str) -> Option<&'a str> {
    if s.len() >= prefix.len() && s.as_bytes()[..prefix.len()].eq_ignore_ascii_case(prefix.as_bytes())
    {
        Some(&s[prefix.len()..])
    } else {
        None
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn chord_basic() {
        let k = parse_chord("CTRL+ALT+DEL").unwrap();
        assert_eq!(k.modifiers, MOD_LCTRL | MOD_LALT);
        assert_eq!(k.key, 0x4C);
    }

    #[test]
    fn chord_case_insensitive() {
        let k = parse_chord("ctrl+shift+a").unwrap();
        assert_eq!(k.modifiers, MOD_LCTRL | MOD_LSHIFT);
        assert_eq!(k.key, 0x04);
    }

    #[test]
    fn chord_modifier_only() {
        let k = parse_chord("CTRL").unwrap();
        assert_eq!(k.modifiers, MOD_LCTRL);
        assert_eq!(k.key, 0);
    }

    #[test]
    fn chord_rejects_two_keys() {
        assert_eq!(parse_chord("A+B"), Err(NameError::InvalidKey));
    }

    #[test]
    fn fkeys() {
        assert_eq!(key_from_name("F1"), Some(0x3A));
        assert_eq!(key_from_name("F12"), Some(0x45));
        assert_eq!(key_from_name("F13"), Some(0x68));
        assert_eq!(key_from_name("F24"), Some(0x73));
        assert_eq!(key_from_name("F25"), None);
    }

    #[test]
    fn gui_aliases() {
        let k = parse_chord("WIN+L").unwrap();
        assert_eq!(k.modifiers, MOD_LGUI);
        assert_eq!(k.key, 0x0F);
    }

    #[test]
    fn mouse_names() {
        assert_eq!(mouse_button_from_name("LEFT"), Ok(MouseButton::Left));
        assert_eq!(mouse_button_from_name("r"), Ok(MouseButton::Right));
        assert_eq!(mouse_button_from_name("nope"), Err(NameError::InvalidMouseButton));
    }

    #[test]
    fn media_names() {
        assert_eq!(media_from_name("PLAY"), Ok(MediaKey::Play));
        assert_eq!(media_from_name("voldown"), Ok(MediaKey::VolDn));
        assert_eq!(media_from_name("bogus"), Err(NameError::InvalidMedia));
    }
}
