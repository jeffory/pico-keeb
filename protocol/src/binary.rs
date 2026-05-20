//! Binary wire protocol.
//!
//! Frame: `[SOF=0xA5] [op:u8] [fixed payload]`. Payload length is implied
//! by the opcode. No length field, no checksum, no byte-stuffing — on
//! desync the decoder returns to `WaitSof` and resynchronises when a
//! fresh `SOF + valid_op` is seen.
//!
//! Reply from firmware: single byte, `ACK=0x06` or `NAK=0x15`.

pub const SOF: u8 = 0xA5;
pub const ACK: u8 = 0x06;
pub const NAK: u8 = 0x15;

pub const OP_KBD: u8 = 0x01;
pub const OP_MOUSE: u8 = 0x02;
pub const OP_CONSUMER: u8 = 0x03;
pub const OP_DELAY: u8 = 0x04;
pub const OP_RESET: u8 = 0x05;

const KBD_LEN: usize = 7;
const MOUSE_LEN: usize = 4;
const CONSUMER_LEN: usize = 2;
const DELAY_LEN: usize = 4;

const MAX_PAYLOAD: usize = KBD_LEN;

/// Maximum encoded frame length on the wire, including SOF + op.
pub const MAX_ENCODED_LEN: usize = 2 + MAX_PAYLOAD;

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum Frame {
    Kbd { modifiers: u8, keys: [u8; 6] },
    Mouse { buttons: u8, dx: i8, dy: i8, wheel: i8 },
    Consumer { usage: u16 },
    Delay { ms: u32 },
    Reset,
}

impl Frame {
    /// Encode into `out` and return the number of bytes written.
    /// `out` must be at least `MAX_ENCODED_LEN` bytes.
    pub fn encode(&self, out: &mut [u8]) -> usize {
        out[0] = SOF;
        match *self {
            Frame::Kbd { modifiers, keys } => {
                out[1] = OP_KBD;
                out[2] = modifiers;
                out[3..9].copy_from_slice(&keys);
                2 + KBD_LEN
            }
            Frame::Mouse { buttons, dx, dy, wheel } => {
                out[1] = OP_MOUSE;
                out[2] = buttons;
                out[3] = dx as u8;
                out[4] = dy as u8;
                out[5] = wheel as u8;
                2 + MOUSE_LEN
            }
            Frame::Consumer { usage } => {
                out[1] = OP_CONSUMER;
                out[2..4].copy_from_slice(&usage.to_le_bytes());
                2 + CONSUMER_LEN
            }
            Frame::Delay { ms } => {
                out[1] = OP_DELAY;
                out[2..6].copy_from_slice(&ms.to_le_bytes());
                2 + DELAY_LEN
            }
            Frame::Reset => {
                out[1] = OP_RESET;
                2
            }
        }
    }
}

/// Streaming byte-wise decoder. Feed bytes; a complete frame appears as
/// `Some(Frame)`. Unknown opcodes silently drop back to SOF search.
pub struct Decoder {
    state: State,
}

enum State {
    WaitSof,
    WaitOp,
    Body { op: u8, need: usize, filled: usize, buf: [u8; MAX_PAYLOAD] },
}

impl Default for Decoder {
    fn default() -> Self {
        Self::new()
    }
}

impl Decoder {
    pub const fn new() -> Self {
        Self { state: State::WaitSof }
    }

    pub fn feed(&mut self, b: u8) -> Option<Frame> {
        match &mut self.state {
            State::WaitSof => {
                if b == SOF {
                    self.state = State::WaitOp;
                }
                None
            }
            State::WaitOp => {
                let need = match b {
                    OP_KBD => KBD_LEN,
                    OP_MOUSE => MOUSE_LEN,
                    OP_CONSUMER => CONSUMER_LEN,
                    OP_DELAY => DELAY_LEN,
                    OP_RESET => {
                        self.state = State::WaitSof;
                        return Some(Frame::Reset);
                    }
                    _ => {
                        // Unknown op → resync. If the stray byte was itself
                        // a SOF, treat it as one so we don't drop a frame.
                        self.state = if b == SOF { State::WaitOp } else { State::WaitSof };
                        return None;
                    }
                };
                self.state = State::Body { op: b, need, filled: 0, buf: [0; MAX_PAYLOAD] };
                None
            }
            State::Body { op, need, filled, buf } => {
                buf[*filled] = b;
                *filled += 1;
                if *filled == *need {
                    let op = *op;
                    let payload = *buf;
                    let n = *need;
                    self.state = State::WaitSof;
                    Some(decode_payload(op, &payload[..n]))
                } else {
                    None
                }
            }
        }
    }
}

fn decode_payload(op: u8, b: &[u8]) -> Frame {
    match op {
        OP_KBD => {
            let mut keys = [0u8; 6];
            keys.copy_from_slice(&b[1..7]);
            Frame::Kbd { modifiers: b[0], keys }
        }
        OP_MOUSE => Frame::Mouse {
            buttons: b[0],
            dx: b[1] as i8,
            dy: b[2] as i8,
            wheel: b[3] as i8,
        },
        OP_CONSUMER => Frame::Consumer {
            usage: u16::from_le_bytes([b[0], b[1]]),
        },
        OP_DELAY => Frame::Delay {
            ms: u32::from_le_bytes([b[0], b[1], b[2], b[3]]),
        },
        _ => unreachable!("decode_payload called with unknown op"),
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    fn roundtrip(f: Frame) {
        let mut buf = [0u8; MAX_ENCODED_LEN];
        let n = f.encode(&mut buf);
        let mut dec = Decoder::new();
        let mut got = None;
        for &b in &buf[..n] {
            if let Some(frame) = dec.feed(b) {
                assert!(got.is_none(), "decoder produced two frames");
                got = Some(frame);
            }
        }
        assert_eq!(got, Some(f));
    }

    #[test]
    fn kbd_roundtrip() {
        roundtrip(Frame::Kbd { modifiers: 0x03, keys: [0x04, 0x05, 0, 0, 0, 0] });
        roundtrip(Frame::Kbd { modifiers: 0, keys: [0; 6] });
        roundtrip(Frame::Kbd { modifiers: 0xFF, keys: [0xFF; 6] });
    }

    #[test]
    fn mouse_roundtrip() {
        roundtrip(Frame::Mouse { buttons: 0x01, dx: 10, dy: -10, wheel: 0 });
        roundtrip(Frame::Mouse { buttons: 0, dx: i8::MIN, dy: i8::MAX, wheel: -1 });
    }

    #[test]
    fn consumer_roundtrip() {
        roundtrip(Frame::Consumer { usage: 0x00E9 });
        roundtrip(Frame::Consumer { usage: 0 });
        roundtrip(Frame::Consumer { usage: 0xFFFF });
    }

    #[test]
    fn delay_roundtrip() {
        roundtrip(Frame::Delay { ms: 0 });
        roundtrip(Frame::Delay { ms: 1000 });
        roundtrip(Frame::Delay { ms: u32::MAX });
    }

    #[test]
    fn reset_roundtrip() {
        roundtrip(Frame::Reset);
    }

    #[test]
    fn encoded_lengths() {
        let mut buf = [0u8; MAX_ENCODED_LEN];
        assert_eq!(Frame::Kbd { modifiers: 0, keys: [0; 6] }.encode(&mut buf), 9);
        assert_eq!(Frame::Mouse { buttons: 0, dx: 0, dy: 0, wheel: 0 }.encode(&mut buf), 6);
        assert_eq!(Frame::Consumer { usage: 0 }.encode(&mut buf), 4);
        assert_eq!(Frame::Delay { ms: 0 }.encode(&mut buf), 6);
        assert_eq!(Frame::Reset.encode(&mut buf), 2);
    }

    #[test]
    fn decoder_ignores_noise_before_sof() {
        let mut dec = Decoder::new();
        for &b in &[0x00, 0xFF, 0x42, 0x7Fu8] {
            assert_eq!(dec.feed(b), None);
        }
        // Now a real frame.
        let mut buf = [0u8; MAX_ENCODED_LEN];
        let f = Frame::Reset;
        let n = f.encode(&mut buf);
        let mut got = None;
        for &b in &buf[..n] {
            if let Some(frame) = dec.feed(b) {
                got = Some(frame);
            }
        }
        assert_eq!(got, Some(f));
    }

    #[test]
    fn decoder_recovers_from_unknown_op() {
        let mut dec = Decoder::new();
        // SOF then bogus op — should reset.
        assert_eq!(dec.feed(SOF), None);
        assert_eq!(dec.feed(0xEE), None);
        // Now a valid frame should still decode.
        let mut buf = [0u8; MAX_ENCODED_LEN];
        let f = Frame::Kbd { modifiers: 0, keys: [0x04, 0, 0, 0, 0, 0] };
        let n = f.encode(&mut buf);
        let mut got = None;
        for &b in &buf[..n] {
            if let Some(frame) = dec.feed(b) {
                got = Some(frame);
            }
        }
        assert_eq!(got, Some(f));
    }

    #[test]
    fn decoder_handles_sof_after_sof() {
        // If a SOF lands where an opcode is expected, don't lose the
        // next byte as the opcode — treat it as the new SOF.
        let mut dec = Decoder::new();
        assert_eq!(dec.feed(SOF), None);
        assert_eq!(dec.feed(SOF), None); // re-sync
        assert_eq!(dec.feed(OP_RESET), Some(Frame::Reset));
    }

    #[test]
    fn decoder_back_to_back_frames() {
        let mut buf = [0u8; MAX_ENCODED_LEN * 2];
        let f1 = Frame::Reset;
        let n1 = f1.encode(&mut buf);
        let f2 = Frame::Delay { ms: 42 };
        let n2 = f2.encode(&mut buf[n1..]);
        let total = n1 + n2;
        let mut dec = Decoder::new();
        let mut got = [None, None];
        let mut i = 0;
        for &b in &buf[..total] {
            if let Some(f) = dec.feed(b) {
                got[i] = Some(f);
                i += 1;
            }
        }
        assert_eq!(got, [Some(f1), Some(f2)]);
    }

    #[test]
    fn decoder_kbd_with_sof_in_payload() {
        // A key with usage 0xA5 in the payload must not be misread as SOF.
        let f = Frame::Kbd { modifiers: SOF, keys: [SOF, 0, 0, 0, 0, 0] };
        roundtrip(f);
    }
}
