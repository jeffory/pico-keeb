# pico-keeb

Serial-driven USB HID (keyboard + mouse + consumer control) firmware for the
Waveshare RP2040-Zero, plus a host CLI that sends commands over a CP2102
USB-UART bridge.

```
[Controlling PC] --USB--> [CP2102] --UART@921600--> [RP2040-Zero] --USB HID--> [Target PC]
    host-cli                                          firmware
```

The RP2040 enumerates on the target PC as a composite HID device
(keyboard + mouse + consumer control) and translates the text commands it
receives on its UART into HID reports. The controlling PC and the target PC
can be the same machine or two different machines.

## Hardware

- Waveshare RP2040-Zero
- CP2102 USB-UART bridge (3.3 V TTL; not 5 V)
- Three jumper wires
- Two USB cables (one per side)

| CP2102 | ↔ | RP2040-Zero |
|--------|---|-------------|
| TXD | → | GP1 (UART0 RX) |
| RXD | → | GP0 (UART0 TX) |
| GND | → | GND |

Power each board from its own USB. The CP2102 attaches to the controlling PC;
the RP2040-Zero attaches to the target PC.

The onboard WS2812 RGB LED is used as a status indicator:

- **dim blue** — idle / ready
- **green flash** — command accepted (`OK`)
- **red flash** — command rejected (`ERR: …`)

## Repository layout

```
pico-keeb/
├── protocol/     # shared parser & command types (no_std + std)
├── firmware/     # RP2040-Zero firmware (Embassy async, excluded from workspace)
└── host-cli/     # controlling-PC CLI (Rust, serialport + clap)
```

The workspace at the repo root contains `protocol` and `host-cli`. The
firmware is a separate crate (different target, different linker flags) and
is built from the `firmware/` directory.

## Building

One-time prerequisites:

```
rustup target add thumbv6m-none-eabi   # installed automatically by rust-toolchain.toml
cargo install elf2uf2-rs               # needed by the firmware runner
```

Build and test the shared & host code:

```
cargo test -p pico-keeb-protocol
cargo build -p pico-keeb-cli --release
```

Build the firmware:

```
cd firmware
cargo build --release
```

Default build has three HID interfaces (keyboard + mouse + consumer) at
`bInterval = 8 ms`. Two Cargo features tune this:

```
cargo build --release --features low-latency   # bInterval=1ms (desktop xHCI)
cargo build --release --features mister        # kbd-only, bInterval=16ms
```

Use `mister` on MiSTer FPGA / DE10-Nano. That board's Synopsys `dwc2` USB
IP through an internal hub trips on multiple concurrently-polled interrupt-IN
endpoints (symptom: recurring `ChHltd set, but reason is unknown` lines in
`dmesg` and a blinking red I/O-board status LED). The `mister` build strips
to a single keyboard interface and raises `bInterval` to 16 ms, which both
the scheduler and the hub's per-port budget tolerate.

## Flashing

1. Hold **BOOTSEL** on the RP2040-Zero, then plug in its USB cable.
   The board mounts as a mass-storage device named `RPI-RP2`.
2. From `firmware/`:
   ```
   cargo run --release
   ```
   `elf2uf2-rs -d` converts the ELF to UF2 and copies it onto the mounted
   volume. The board reboots into the new firmware automatically.
3. Unplug BOOTSEL is not needed — `RPI-RP2` disappears once flashing completes.

On first boot the firmware emits a one-line banner on the UART:

```
pico-keeb v0.1.0 ready
```

Seeing this in a plain terminal (`picocom -b 921600 /dev/ttyUSB0`) confirms
the RP2040→CP2102 direction is wired correctly.

## Using the CLI

```
pico-keeb-cli --port /dev/ttyUSB0 type "hello world"
pico-keeb-cli --port /dev/ttyUSB0 key CTRL+ALT+DEL
pico-keeb-cli --port /dev/ttyUSB0 key F5
pico-keeb-cli --port /dev/ttyUSB0 hold SHIFT
pico-keeb-cli --port /dev/ttyUSB0 type "THIS IS SHOUTED"
pico-keeb-cli --port /dev/ttyUSB0 release SHIFT
pico-keeb-cli --port /dev/ttyUSB0 move 100 0
pico-keeb-cli --port /dev/ttyUSB0 click left
pico-keeb-cli --port /dev/ttyUSB0 scroll -3
pico-keeb-cli --port /dev/ttyUSB0 media volup
pico-keeb-cli --port /dev/ttyUSB0 delay 500
pico-keeb-cli --port /dev/ttyUSB0 reset
pico-keeb-cli --port /dev/ttyUSB0 script macros.txt
pico-keeb-cli --port /dev/ttyUSB0 repl
pico-keeb-cli --port /dev/ttyUSB0 realtime
```

Each subcommand sends one protocol line and waits for the firmware's
`OK\n` or `ERR: <reason>\n` reply. On `ERR:` the CLI exits non-zero with
the reason. Pass `-v` to also print `OK` replies.

### Realtime mode

`realtime` puts the terminal in raw mode and forwards every keystroke to
the target PC as it's pressed. The typing never reaches the controlling
terminal — it lands on the target.

```
$ pico-keeb-cli --port /dev/ttyUSB0 realtime
pico-keeb realtime — Ctrl+] or double-Esc to exit
```

Mapping:

- Plain characters (including shifted symbols like `!` or `@`) → `TYPE <c>`;
  the firmware's US-QWERTY keymap handles the shift.
- Chords with Ctrl/Alt/Super modifiers (e.g. `Ctrl+L`, `Alt+Tab`, `Cmd+Space`)
  → `KEY <mods>+<key>`.
- Named keys (Enter, Tab, Backspace, Esc, arrows, Home/End, PageUp/Down,
  Insert, Delete, F1–F12) → `KEY <name>` with any modifiers applied.

**Exit:** press **Ctrl+]**, or tap **Esc twice within 750 ms** (useful on
terminals like Ghostty that may intercept Ctrl+]). Every other key —
including Ctrl+C — is forwarded to the target. The terminal is restored
automatically even on panic (via a `Drop` guard). Pass `--debug-keys` to
print each received key event to stderr.

Focus is per-terminal only; keys are captured while this CLI's terminal
window is focused. For a system-wide hook you'd need a global-key
library and elevated permissions, which this mode deliberately avoids.

### Script files

`script <file>` sends one protocol line per file line. Blank lines and lines
starting with `#` are ignored. Example `macros.txt`:

```
# open a terminal
KEY LGUI+T
DELAY 400
TYPE echo hello
KEY ENTER
```

## Wire protocol

One command per line, `\n`-terminated, case-insensitive, ASCII.

| Command | Arguments | Notes |
|---|---|---|
| `TYPE <text>` | rest of line, verbatim | US layout; unmappable bytes → `ERR` |
| `KEY <combo>` | e.g. `CTRL+ALT+DEL`, `F5`, `ENTER` | press+release |
| `HOLD <combo>` / `RELEASE <combo>` | same | no auto-release |
| `MOVE <dx> <dy>` | signed i16 each | firmware chunks into i8 HID reports |
| `CLICK <LEFT\|RIGHT\|MIDDLE>` | — | down + up |
| `SCROLL <n>` | signed i8 | vertical wheel |
| `MEDIA <PLAY\|PAUSE\|NEXT\|PREV\|VOLUP\|VOLDN\|MUTE>` | — | consumer control |
| `DELAY <ms>` | u32 | pause command processing; firmware clamps to 5000 ms (chunk longer pauses on the host) |
| `RESET` | — | release all keys, modifiers, buttons |

Recognised modifier names: `CTRL`, `SHIFT`, `ALT`, `GUI` (and `WIN`, `CMD`,
`META`, `SUPER`) with `L`/`R` prefixes for explicit sides; `ALTGR` = `RALT`.

Recognised key names (non-exhaustive): `A`–`Z`, `0`–`9`, `F1`–`F24`,
`ENTER`/`RETURN`, `ESC`/`ESCAPE`, `BACKSPACE`/`BS`, `TAB`, `SPACE`,
arrows (`UP`/`DOWN`/`LEFT`/`RIGHT`), `HOME`/`END`/`PAGEUP`/`PAGEDOWN`,
`INSERT`/`DELETE`, `CAPSLOCK`, `PRINTSCREEN`/`PRTSC`, `PAUSE`, `MENU`,
and all printable symbols as their punctuation names (`COMMA`, `PERIOD`, …).

Replies:

- `OK\n` — command accepted and executed
- `ERR: <reason>\n` — parse error, unmappable byte, etc.

## Troubleshooting

**Nothing happens when the CLI runs — timeout on reply**
Connect a plain terminal (`picocom -b 921600 /dev/ttyUSB0`) and reset the
RP2040. If you don't see `pico-keeb ready`, the RP2040→CP2102 direction
is broken: check GP0 wiring, shared GND, and that the CP2102 is set to
3.3 V. If you *do* see the banner but commands still time out, the
CP2102→RP2040 direction is broken: check GP1 wiring and that you're on
the CP2102's **TXD** pin (not a status-LED pin with a similar label).

**CLI commands ACK but nothing types on the target**
The RP2040's USB-HID side isn't enumerated. Check `lsusb | grep 16c0` on
the target PC — you should see `16c0:27db pico-keeb`. If nothing, plug
the RP2040's USB-C into the target PC. The firmware will silently drop
HID reports when no target is present (rather than stalling the wire
protocol), so ACKs arrive normally even when typing goes nowhere.

**Only Escape (or a handful of keys) registers on the target**
Some input stacks — notably MiSTer FPGA — filter keypresses shorter than
~20 ms as chatter. By default the CLI sends press and release back-to-back
with only a ~1 HID-poll gap between them. Pass `--key-hold-ms 30` (or
higher) to insert a DELAY frame between press and release so each tap
holds long enough to register:

```
pico-keeb-cli --port /dev/ttyUSB0 --key-hold-ms 30 key DOWN
pico-keeb-cli --port /dev/ttyUSB0 --key-hold-ms 30 realtime
```

**Garbled output in the terminal**
Baud mismatch. Confirm firmware is at 921600 (the default) and the
terminal/CLI are too. Some CP2102 clones don't hold 921600 — drop both
sides to 115200 if needed (change `uart_cfg.baudrate` in
`firmware/src/main.rs` and pass `--baud 115200` to the CLI).

**Typing produces wrong characters on the target**
The firmware assumes a US-QWERTY layout on the target PC. Set the target's
keyboard layout to US, or remap the affected characters yourself.

**Target doesn't respond to `MEDIA play` but `MEDIA pause` works (or vice-versa)**
Some OSes only listen for the combined Play/Pause toggle (consumer usage
`0xCD`). Open an issue / ask and I can add a `PLAYPAUSE` variant.

## Design doc

See `/home/keith/.claude/plans/curried-inventing-corbato.md` for the full
design record (protocol decisions, task topology, HID descriptor choices).

## License

Dual-licensed under MIT or Apache-2.0 at your option.
