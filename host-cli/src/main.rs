use std::fs;
use std::io::{BufRead, Read, Write};
use std::path::{Path, PathBuf};
use std::time::{Duration, Instant};

use anyhow::{anyhow, Context, Result};
use clap::{Parser, Subcommand};
use pico_keeb_protocol::binary::{Frame, ACK, MAX_ENCODED_LEN};
use pico_keeb_protocol::keymap;
use pico_keeb_protocol::names;
use pico_keeb_protocol::{KeyChord, MouseButton, MOD_LSHIFT};
use serialport::SerialPort;

#[derive(Parser)]
#[command(version, about = "pico-keeb host CLI — send HID frames over serial")]
struct Cli {
    /// Serial port path, e.g. /dev/ttyUSB0 or COM3
    #[arg(long, short)]
    port: String,
    /// UART baud rate (must match firmware)
    #[arg(long, short, default_value_t = 921_600)]
    baud: u32,
    /// Per-frame ACK timeout in milliseconds
    #[arg(long, default_value_t = 1_000)]
    timeout_ms: u64,
    /// Fire-and-forget: don't wait for per-frame ACKs (fastest; no flow control)
    #[arg(long)]
    no_ack: bool,
    /// Hold each key down for this many ms before releasing. Raise for hosts
    /// that filter short keypresses (e.g. `--key-hold-ms 30` for MiSTer FPGA).
    #[arg(long, default_value_t = 0)]
    key_hold_ms: u32,
    #[command(subcommand)]
    cmd: Cmd,
}

#[derive(Subcommand)]
enum Cmd {
    /// Type literal text (US layout, ASCII)
    Type {
        #[arg(trailing_var_arg = true, allow_hyphen_values = true, required = true)]
        text: Vec<String>,
    },
    /// Press a key chord once, e.g. CTRL+ALT+DEL, F5, ENTER
    Key { combo: String },
    /// Press and hold a chord (persists across invocations via /tmp state)
    Hold { combo: String },
    /// Release a previously-held chord
    Release { combo: String },
    /// Relative mouse move (signed)
    Move { dx: i16, dy: i16 },
    /// Click a mouse button (left|right|middle)
    Click { button: String },
    /// Scroll wheel by N (signed, -127..=127)
    Scroll { n: i16 },
    /// Consumer/media key (play|pause|next|prev|volup|voldn|mute)
    Media { which: String },
    /// Pause firmware command processing
    Delay { ms: u32 },
    /// Release everything and clear held state
    Reset,
    /// Send commands from a script file, one per line
    Script { file: PathBuf },
    /// Interactive prompt for protocol subcommands
    Repl,
    /// Forward keystrokes from this terminal to the target in real time (Ctrl+] or double-Esc to exit)
    Realtime {
        /// Print each received key event to stderr (for diagnosing terminal-specific binding issues)
        #[arg(long)]
        debug_keys: bool,
    },
    /// Wire-latency benchmark: time N round-trips
    Bench {
        #[arg(long, default_value_t = 1000)]
        count: u32,
    },
}

fn main() -> Result<()> {
    let cli = Cli::parse();
    let port = serialport::new(&cli.port, cli.baud)
        .timeout(Duration::from_millis(cli.timeout_ms))
        .data_bits(serialport::DataBits::Eight)
        .parity(serialport::Parity::None)
        .stop_bits(serialport::StopBits::One)
        .flow_control(serialport::FlowControl::None)
        .open()
        .with_context(|| format!("opening {}", cli.port))?;

    let _ = port.clear(serialport::ClearBuffer::Input);

    let mut link = Link::new(port, cli.no_ack, cli.key_hold_ms);
    let mut state = HeldState::load(&cli.port);

    match cli.cmd {
        Cmd::Type { text } => {
            let joined = text.join(" ");
            type_text(&mut link, &state, &joined)?;
        }
        Cmd::Key { combo } => {
            let chord = names::parse_chord(&combo).map_err(|e| anyhow!("{e}"))?;
            tap_chord(&mut link, &state, chord)?;
        }
        Cmd::Hold { combo } => {
            let chord = names::parse_chord(&combo).map_err(|e| anyhow!("{e}"))?;
            state.hold(chord);
            state.save(&cli.port)?;
            link.send(&kbd_report(&state))?;
        }
        Cmd::Release { combo } => {
            let chord = names::parse_chord(&combo).map_err(|e| anyhow!("{e}"))?;
            state.release(chord);
            state.save(&cli.port)?;
            link.send(&kbd_report(&state))?;
        }
        Cmd::Move { dx, dy } => mouse_move(&mut link, dx, dy)?,
        Cmd::Click { button } => {
            let btn = names::mouse_button_from_name(&button).map_err(|e| anyhow!("{e}"))?;
            click(&mut link, btn)?;
        }
        Cmd::Scroll { n } => {
            if !(-127..=127).contains(&n) {
                return Err(anyhow!("scroll out of range (-127..=127)"));
            }
            link.send(&Frame::Mouse { buttons: 0, dx: 0, dy: 0, wheel: n as i8 })?;
        }
        Cmd::Media { which } => {
            let m = names::media_from_name(&which).map_err(|e| anyhow!("{e}"))?;
            link.send(&Frame::Consumer { usage: m.usage() })?;
            link.hold_between_press_and_release()?;
            link.send(&Frame::Consumer { usage: 0 })?;
        }
        Cmd::Delay { ms } => link.send(&Frame::Delay { ms })?,
        Cmd::Reset => {
            state = HeldState::default();
            state.save(&cli.port)?;
            link.send(&Frame::Reset)?;
        }
        Cmd::Script { file } => run_script(&mut link, &mut state, &cli.port, &file)?,
        Cmd::Repl => run_repl(&mut link, &mut state, &cli.port)?,
        Cmd::Realtime { debug_keys } => run_realtime(&mut link, &state, debug_keys)?,
        Cmd::Bench { count } => run_bench(&mut link, count)?,
    }
    Ok(())
}

// ---- Link (wire-level I/O) ----

struct Link {
    port: Box<dyn SerialPort>,
    no_ack: bool,
    key_hold_ms: u32,
}

impl Link {
    fn new(port: Box<dyn SerialPort>, no_ack: bool, key_hold_ms: u32) -> Self {
        Self { port, no_ack, key_hold_ms }
    }

    /// If `--key-hold-ms` is non-zero, send a DELAY frame so a queued press
    /// stays visible on the wire long enough for the target host to register
    /// it as a real keypress (some input stacks, e.g. MiSTer, filter taps
    /// shorter than ~20 ms).
    fn hold_between_press_and_release(&mut self) -> Result<()> {
        if self.key_hold_ms > 0 {
            self.send(&Frame::Delay { ms: self.key_hold_ms })?;
        }
        Ok(())
    }

    fn send(&mut self, frame: &Frame) -> Result<()> {
        self.send_raw(frame)?;
        if !self.no_ack {
            self.await_ack()?;
        }
        Ok(())
    }

    fn send_raw(&mut self, frame: &Frame) -> Result<()> {
        let mut buf = [0u8; MAX_ENCODED_LEN];
        let n = frame.encode(&mut buf);
        self.port.write_all(&buf[..n])?;
        Ok(())
    }

    fn await_ack(&mut self) -> Result<()> {
        let mut b = [0u8; 1];
        loop {
            self.port.read_exact(&mut b).context("reading ack")?;
            match b[0] {
                ACK => return Ok(()),
                // Tolerate stray bytes (e.g. the boot banner on first connect).
                _ => continue,
            }
        }
    }
}

// ---- Held state (persisted per-port under /tmp) ----

#[derive(Default, Clone, Copy)]
struct HeldState {
    modifiers: u8,
    keys: [u8; 6],
}

impl HeldState {
    fn load(port: &str) -> Self {
        match fs::read(state_path(port)) {
            Ok(bytes) if bytes.len() == 7 => HeldState {
                modifiers: bytes[0],
                keys: [bytes[1], bytes[2], bytes[3], bytes[4], bytes[5], bytes[6]],
            },
            _ => HeldState::default(),
        }
    }

    fn save(&self, port: &str) -> Result<()> {
        let mut bytes = [0u8; 7];
        bytes[0] = self.modifiers;
        bytes[1..7].copy_from_slice(&self.keys);
        fs::write(state_path(port), bytes).context("saving held state")?;
        Ok(())
    }

    fn hold(&mut self, chord: KeyChord) {
        self.modifiers |= chord.modifiers;
        if chord.key != 0 && !self.keys.contains(&chord.key) {
            if let Some(slot) = self.keys.iter_mut().find(|s| **s == 0) {
                *slot = chord.key;
            }
        }
    }

    fn release(&mut self, chord: KeyChord) {
        self.modifiers &= !chord.modifiers;
        if chord.key != 0 {
            for slot in self.keys.iter_mut() {
                if *slot == chord.key {
                    *slot = 0;
                }
            }
            let mut compact = [0u8; 6];
            let mut j = 0;
            for &k in self.keys.iter() {
                if k != 0 {
                    compact[j] = k;
                    j += 1;
                }
            }
            self.keys = compact;
        }
    }
}

fn state_path(port: &str) -> PathBuf {
    let name = Path::new(port)
        .file_name()
        .and_then(|s| s.to_str())
        .unwrap_or("default")
        .replace(|c: char| !c.is_ascii_alphanumeric() && c != '-' && c != '_', "_");
    PathBuf::from(format!("/tmp/pico-keeb-{name}.state"))
}

fn kbd_report(state: &HeldState) -> Frame {
    Frame::Kbd { modifiers: state.modifiers, keys: state.keys }
}

fn compose_keys(held: [u8; 6], add: u8) -> [u8; 6] {
    let mut out = held;
    if add != 0 && !held.contains(&add) {
        for slot in out.iter_mut() {
            if *slot == 0 {
                *slot = add;
                break;
            }
        }
    }
    out
}

// ---- High-level command helpers ----

fn tap_chord(link: &mut Link, held: &HeldState, chord: KeyChord) -> Result<()> {
    let mods = held.modifiers | chord.modifiers;
    let keys = compose_keys(held.keys, chord.key);
    link.send(&Frame::Kbd { modifiers: mods, keys })?;
    link.hold_between_press_and_release()?;
    link.send(&kbd_report(held))?;
    Ok(())
}

fn type_text(link: &mut Link, held: &HeldState, text: &str) -> Result<()> {
    for &b in text.as_bytes() {
        let (usage, shift) = keymap::ascii_to_hid(b)
            .ok_or_else(|| anyhow!("unmappable byte 0x{b:02X}"))?;
        let mods = held.modifiers | if shift { MOD_LSHIFT } else { 0 };
        let keys = compose_keys(held.keys, usage);
        link.send(&Frame::Kbd { modifiers: mods, keys })?;
        link.hold_between_press_and_release()?;
        link.send(&kbd_report(held))?;
    }
    Ok(())
}

fn mouse_move(link: &mut Link, dx: i16, dy: i16) -> Result<()> {
    let (mut rx, mut ry) = (dx, dy);
    while rx != 0 || ry != 0 {
        let cx = rx.clamp(-127, 127) as i8;
        let cy = ry.clamp(-127, 127) as i8;
        link.send(&Frame::Mouse { buttons: 0, dx: cx, dy: cy, wheel: 0 })?;
        rx -= cx as i16;
        ry -= cy as i16;
    }
    Ok(())
}

fn click(link: &mut Link, btn: MouseButton) -> Result<()> {
    link.send(&Frame::Mouse { buttons: btn.mask(), dx: 0, dy: 0, wheel: 0 })?;
    link.hold_between_press_and_release()?;
    link.send(&Frame::Mouse { buttons: 0, dx: 0, dy: 0, wheel: 0 })?;
    Ok(())
}

// ---- Script / REPL / Realtime ----

fn run_script(link: &mut Link, state: &mut HeldState, port_name: &str, path: &Path) -> Result<()> {
    let text = fs::read_to_string(path)
        .with_context(|| format!("reading script {}", path.display()))?;
    for (lineno, raw) in text.lines().enumerate() {
        let line = raw.trim();
        if line.is_empty() || line.starts_with('#') {
            continue;
        }
        dispatch_line(link, state, port_name, line)
            .with_context(|| format!("{}:{}: {}", path.display(), lineno + 1, line))?;
    }
    Ok(())
}

fn run_repl(link: &mut Link, state: &mut HeldState, port_name: &str) -> Result<()> {
    let stdin = std::io::stdin();
    let mut stdout = std::io::stdout();
    let mut line = String::new();
    loop {
        write!(stdout, "pico> ")?;
        stdout.flush()?;
        line.clear();
        if stdin.lock().read_line(&mut line)? == 0 {
            break;
        }
        let trimmed = line.trim();
        if trimmed.is_empty() {
            continue;
        }
        if trimmed == "quit" || trimmed == "exit" {
            break;
        }
        if let Err(e) = dispatch_line(link, state, port_name, trimmed) {
            eprintln!("  error: {e:#}");
        }
    }
    Ok(())
}

/// Parse and execute one high-level line (used by both script and repl).
/// Syntax matches the old ASCII protocol for backward compatibility.
fn dispatch_line(
    link: &mut Link,
    state: &mut HeldState,
    port_name: &str,
    line: &str,
) -> Result<()> {
    let (cmd, rest) = match line.find([' ', '\t']) {
        Some(i) => (&line[..i], line[i + 1..].trim_start()),
        None => (line, ""),
    };
    match cmd.to_ascii_uppercase().as_str() {
        "TYPE" => type_text(link, state, rest),
        "KEY" => {
            let chord = names::parse_chord(rest).map_err(|e| anyhow!("{e}"))?;
            tap_chord(link, state, chord)
        }
        "HOLD" => {
            let chord = names::parse_chord(rest).map_err(|e| anyhow!("{e}"))?;
            state.hold(chord);
            state.save(port_name)?;
            link.send(&kbd_report(state))
        }
        "RELEASE" => {
            let chord = names::parse_chord(rest).map_err(|e| anyhow!("{e}"))?;
            state.release(chord);
            state.save(port_name)?;
            link.send(&kbd_report(state))
        }
        "MOVE" => {
            let mut it = rest.split_ascii_whitespace();
            let dx: i16 = it.next().ok_or_else(|| anyhow!("missing dx"))?.parse()?;
            let dy: i16 = it.next().ok_or_else(|| anyhow!("missing dy"))?.parse()?;
            if it.next().is_some() {
                return Err(anyhow!("extra arguments"));
            }
            mouse_move(link, dx, dy)
        }
        "CLICK" => {
            let btn = names::mouse_button_from_name(rest.trim()).map_err(|e| anyhow!("{e}"))?;
            click(link, btn)
        }
        "SCROLL" => {
            let n: i16 = rest.trim().parse()?;
            if !(-127..=127).contains(&n) {
                return Err(anyhow!("scroll out of range"));
            }
            link.send(&Frame::Mouse { buttons: 0, dx: 0, dy: 0, wheel: n as i8 })
        }
        "MEDIA" => {
            let m = names::media_from_name(rest.trim()).map_err(|e| anyhow!("{e}"))?;
            link.send(&Frame::Consumer { usage: m.usage() })?;
            link.hold_between_press_and_release()?;
            link.send(&Frame::Consumer { usage: 0 })
        }
        "DELAY" => {
            let ms: u32 = rest.trim().parse()?;
            link.send(&Frame::Delay { ms })
        }
        "RESET" => {
            *state = HeldState::default();
            state.save(port_name)?;
            link.send(&Frame::Reset)
        }
        _ => Err(anyhow!("unknown command: {cmd}")),
    }
}

struct RawModeGuard;

impl RawModeGuard {
    fn new() -> Result<Self> {
        crossterm::terminal::enable_raw_mode().context("entering raw mode")?;
        Ok(Self)
    }
}

impl Drop for RawModeGuard {
    fn drop(&mut self) {
        let _ = crossterm::terminal::disable_raw_mode();
    }
}

fn run_realtime(link: &mut Link, held: &HeldState, debug_keys: bool) -> Result<()> {
    use crossterm::event::{self, Event, KeyCode, KeyEventKind};

    eprintln!("pico-keeb realtime — Ctrl+] or double-Esc to exit");

    let guard = RawModeGuard::new()?;

    // Double-Esc exit: track the previous Esc press timestamp. Two Esc
    // presses within DOUBLE_ESC_WINDOW exit cleanly — a terminal-agnostic
    // fallback for emulators that swallow Ctrl+].
    const DOUBLE_ESC_WINDOW: Duration = Duration::from_millis(750);
    let mut last_esc: Option<Instant> = None;

    loop {
        let Event::Key(ke) = event::read()? else {
            continue;
        };
        if matches!(ke.kind, KeyEventKind::Release | KeyEventKind::Repeat) {
            continue;
        }
        if debug_keys {
            eprint!("\r\nkey: {:?}\r\n", ke);
        }
        if is_exit_key(&ke) {
            break;
        }
        if ke.code == KeyCode::Esc {
            let now = Instant::now();
            if last_esc.is_some_and(|t| now.duration_since(t) < DOUBLE_ESC_WINDOW) {
                break;
            }
            last_esc = Some(now);
        } else {
            last_esc = None;
        }
        if let Err(e) = handle_realtime_event(link, held, &ke) {
            eprint!("\r\nerror: {e}\r\n");
        }
    }

    drop(guard);
    eprintln!("\rexited realtime");
    Ok(())
}

fn is_exit_key(ke: &crossterm::event::KeyEvent) -> bool {
    use crossterm::event::{KeyCode, KeyModifiers};
    let ctrl_only = ke.modifiers == KeyModifiers::CONTROL;
    // crossterm 0.28's Unix parser maps the Ctrl+] byte (0x1D) to
    // Char('5') + CONTROL (via `c - 0x1C + b'4'`). On some terminals with
    // the Kitty keyboard protocol active it instead comes through as
    // Char(']') + CONTROL, which is the "expected" form.
    if ctrl_only && matches!(ke.code, KeyCode::Char(']') | KeyCode::Char('5')) {
        return true;
    }
    // Fallback: raw GS byte if some terminal surfaces it directly.
    if ke.code == KeyCode::Char('\x1d') {
        return true;
    }
    false
}

fn handle_realtime_event(
    link: &mut Link,
    held: &HeldState,
    ke: &crossterm::event::KeyEvent,
) -> Result<()> {
    use crossterm::event::{KeyCode, KeyModifiers};

    let mut mods = held.modifiers;
    if ke.modifiers.contains(KeyModifiers::CONTROL) {
        mods |= pico_keeb_protocol::MOD_LCTRL;
    }
    if ke.modifiers.contains(KeyModifiers::ALT) {
        mods |= pico_keeb_protocol::MOD_LALT;
    }
    if ke.modifiers.contains(KeyModifiers::SUPER) || ke.modifiers.contains(KeyModifiers::META) {
        mods |= pico_keeb_protocol::MOD_LGUI;
    }
    let command_mod_active = mods != held.modifiers;
    let shift_mod = ke.modifiers.contains(KeyModifiers::SHIFT);

    let named_key = match ke.code {
        KeyCode::Enter => Some(0x28),
        KeyCode::Tab => Some(0x2B),
        KeyCode::BackTab => Some(0x2B),
        KeyCode::Backspace => Some(0x2A),
        KeyCode::Esc => Some(0x29),
        KeyCode::Left => Some(0x50),
        KeyCode::Right => Some(0x4F),
        KeyCode::Up => Some(0x52),
        KeyCode::Down => Some(0x51),
        KeyCode::Home => Some(0x4A),
        KeyCode::End => Some(0x4D),
        KeyCode::PageUp => Some(0x4B),
        KeyCode::PageDown => Some(0x4E),
        KeyCode::Insert => Some(0x49),
        KeyCode::Delete => Some(0x4C),
        KeyCode::F(n) if (1..=12).contains(&n) => Some(0x3A + (n - 1)),
        KeyCode::F(n) if (13..=24).contains(&n) => Some(0x68 + (n - 13)),
        _ => None,
    };

    if let Some(usage) = named_key {
        let mut m = mods;
        if shift_mod || matches!(ke.code, KeyCode::BackTab) {
            m |= MOD_LSHIFT;
        }
        let keys = compose_keys(held.keys, usage);
        link.send(&Frame::Kbd { modifiers: m, keys })?;
        link.hold_between_press_and_release()?;
        link.send(&kbd_report(held))?;
        return Ok(());
    }

    if let KeyCode::Char(c) = ke.code {
        if command_mod_active && c.is_ascii_alphanumeric() {
            let usage = if c.is_ascii_alphabetic() {
                0x04 + (c.to_ascii_uppercase() as u8 - b'A')
            } else if c == '0' {
                0x27
            } else {
                0x1E + (c as u8 - b'1')
            };
            let mut m = mods;
            if shift_mod {
                m |= MOD_LSHIFT;
            }
            let keys = compose_keys(held.keys, usage);
            link.send(&Frame::Kbd { modifiers: m, keys })?;
            link.hold_between_press_and_release()?;
            link.send(&kbd_report(held))?;
            return Ok(());
        }
        // Plain char → let the firmware-keymap-equivalent on host expand shift.
        let Some((usage, shift)) = keymap::ascii_to_hid(c as u8) else {
            return Ok(());
        };
        let mut m = held.modifiers;
        if shift {
            m |= MOD_LSHIFT;
        }
        let keys = compose_keys(held.keys, usage);
        link.send(&Frame::Kbd { modifiers: m, keys })?;
        link.hold_between_press_and_release()?;
        link.send(&kbd_report(held))?;
    }
    Ok(())
}

// ---- Benchmark ----

fn run_bench(link: &mut Link, count: u32) -> Result<()> {
    if link.no_ack {
        return run_bench_throughput(link, count);
    }

    println!("pico-keeb bench: {count} round-trips via DELAY(0)");
    // Warm-up
    for _ in 0..16 {
        link.send(&Frame::Delay { ms: 0 })?;
    }

    let mut samples: Vec<u128> = Vec::with_capacity(count as usize);
    let total_start = Instant::now();
    for _ in 0..count {
        let t0 = Instant::now();
        link.send(&Frame::Delay { ms: 0 })?;
        samples.push(t0.elapsed().as_nanos());
    }
    let total = total_start.elapsed();

    samples.sort_unstable();
    let mean_ns = samples.iter().sum::<u128>() / samples.len() as u128;
    let p50 = samples[samples.len() / 2];
    let p95 = samples[(samples.len() * 95) / 100];
    let p99 = samples[(samples.len() * 99) / 100];
    let max = *samples.last().unwrap();
    let min = *samples.first().unwrap();

    println!("  total elapsed : {:.3} ms", total.as_secs_f64() * 1e3);
    println!("  rate          : {:.1} frames/s", count as f64 / total.as_secs_f64());
    println!("  round-trip    : min {:.1} µs | p50 {:.1} µs | mean {:.1} µs",
        min as f64 / 1e3, p50 as f64 / 1e3, mean_ns as f64 / 1e3);
    println!("                  p95 {:.1} µs | p99 {:.1} µs | max {:.1} µs",
        p95 as f64 / 1e3, p99 as f64 / 1e3, max as f64 / 1e3);
    Ok(())
}

fn run_bench_throughput(link: &mut Link, count: u32) -> Result<()> {
    println!("pico-keeb bench (streaming, --no-ack): {count} frames");
    let t0 = Instant::now();
    for _ in 0..count {
        link.send_raw(&Frame::Delay { ms: 0 })?;
    }
    link.port.flush()?;
    let elapsed = t0.elapsed();
    println!("  elapsed : {:.3} ms", elapsed.as_secs_f64() * 1e3);
    println!("  rate    : {:.1} frames/s", count as f64 / elapsed.as_secs_f64());
    println!("  note    : fire-and-forget; firmware-side latency not measured");
    Ok(())
}
