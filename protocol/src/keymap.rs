/// US-QWERTY ASCII → (HID keyboard usage id, shift-required) mapping.
///
/// Returns `None` for bytes that cannot be typed as a single keypress
/// under US layout (e.g. most control chars).
pub fn ascii_to_hid(c: u8) -> Option<(u8, bool)> {
    match c {
        b'a'..=b'z' => Some((0x04 + (c - b'a'), false)),
        b'A'..=b'Z' => Some((0x04 + (c - b'A'), true)),
        b'1'..=b'9' => Some((0x1E + (c - b'1'), false)),
        b'0' => Some((0x27, false)),
        b'!' => Some((0x1E, true)),
        b'@' => Some((0x1F, true)),
        b'#' => Some((0x20, true)),
        b'$' => Some((0x21, true)),
        b'%' => Some((0x22, true)),
        b'^' => Some((0x23, true)),
        b'&' => Some((0x24, true)),
        b'*' => Some((0x25, true)),
        b'(' => Some((0x26, true)),
        b')' => Some((0x27, true)),
        b'\n' => Some((0x28, false)), // Enter
        b'\t' => Some((0x2B, false)),
        b' ' => Some((0x2C, false)),
        b'-' => Some((0x2D, false)),
        b'_' => Some((0x2D, true)),
        b'=' => Some((0x2E, false)),
        b'+' => Some((0x2E, true)),
        b'[' => Some((0x2F, false)),
        b'{' => Some((0x2F, true)),
        b']' => Some((0x30, false)),
        b'}' => Some((0x30, true)),
        b'\\' => Some((0x31, false)),
        b'|' => Some((0x31, true)),
        b';' => Some((0x33, false)),
        b':' => Some((0x33, true)),
        b'\'' => Some((0x34, false)),
        b'"' => Some((0x34, true)),
        b'`' => Some((0x35, false)),
        b'~' => Some((0x35, true)),
        b',' => Some((0x36, false)),
        b'<' => Some((0x36, true)),
        b'.' => Some((0x37, false)),
        b'>' => Some((0x37, true)),
        b'/' => Some((0x38, false)),
        b'?' => Some((0x38, true)),
        _ => None,
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn letters() {
        assert_eq!(ascii_to_hid(b'a'), Some((0x04, false)));
        assert_eq!(ascii_to_hid(b'z'), Some((0x1D, false)));
        assert_eq!(ascii_to_hid(b'A'), Some((0x04, true)));
        assert_eq!(ascii_to_hid(b'Z'), Some((0x1D, true)));
    }

    #[test]
    fn digits() {
        assert_eq!(ascii_to_hid(b'1'), Some((0x1E, false)));
        assert_eq!(ascii_to_hid(b'9'), Some((0x26, false)));
        assert_eq!(ascii_to_hid(b'0'), Some((0x27, false)));
    }

    #[test]
    fn shifted_symbols() {
        assert_eq!(ascii_to_hid(b'!'), Some((0x1E, true)));
        assert_eq!(ascii_to_hid(b')'), Some((0x27, true)));
        assert_eq!(ascii_to_hid(b'?'), Some((0x38, true)));
    }

    #[test]
    fn whitespace_and_control() {
        assert_eq!(ascii_to_hid(b' '), Some((0x2C, false)));
        assert_eq!(ascii_to_hid(b'\t'), Some((0x2B, false)));
        assert_eq!(ascii_to_hid(b'\n'), Some((0x28, false)));
    }

    #[test]
    fn unmappable() {
        assert_eq!(ascii_to_hid(0x00), None);
        assert_eq!(ascii_to_hid(0x7F), None);
        assert_eq!(ascii_to_hid(0x80), None);
    }

    #[test]
    fn all_printable_ascii_covered() {
        for c in 0x20u8..=0x7Eu8 {
            assert!(ascii_to_hid(c).is_some(), "missing mapping for {:#x}", c);
        }
    }
}
