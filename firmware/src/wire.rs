use defmt::warn;
use embassy_rp::peripherals::USB;
use embassy_rp::uart::BufferedUart;
use embassy_rp::usb::Driver as UsbDriver;
use embassy_sync::blocking_mutex::raw::CriticalSectionRawMutex;
use embassy_sync::channel::Channel;
use embassy_time::Timer;
use embassy_usb::class::hid::HidWriter;
use embedded_io_async::{Read, Write};
use pico_keeb_protocol::binary::{Decoder, Frame, ACK, NAK};
use usbd_hid::descriptor::KeyboardReport;
#[cfg(not(feature = "mister"))]
use usbd_hid::descriptor::{MediaKeyboardReport, MouseReport};

use crate::led::{LedEvent, LED_SIG};

type UsbDriverT = UsbDriver<'static, USB>;

pub type KbdWriter = HidWriter<'static, UsbDriverT, 8>;
#[cfg(not(feature = "mister"))]
pub type MouseWriter = HidWriter<'static, UsbDriverT, 8>;
#[cfg(not(feature = "mister"))]
pub type ConsumerWriter = HidWriter<'static, UsbDriverT, 2>;

// HID reports are queued through bounded channels so the UART task can
// reply to each frame the instant it's decoded, without waiting for the
// USB host to poll the IN endpoint.
pub static KBD_CHAN: Channel<CriticalSectionRawMutex, KeyboardReport, 32> = Channel::new();
#[cfg(not(feature = "mister"))]
pub static MOUSE_CHAN: Channel<CriticalSectionRawMutex, MouseReport, 32> = Channel::new();
#[cfg(not(feature = "mister"))]
pub static CONSUMER_CHAN: Channel<CriticalSectionRawMutex, MediaKeyboardReport, 16> =
    Channel::new();

/// Largest DELAY the firmware will honour in a single frame. Longer waits
/// would starve the UART task and overflow its 1024-byte RX buffer at
/// 921600 baud (~83 ms to fill). The host must chunk longer waits itself.
const MAX_DELAY_MS: u32 = 5_000;

#[embassy_executor::task]
pub async fn uart_task(mut uart: BufferedUart) {
    let _ = uart.write_all(b"pico-keeb ready\n").await;
    let _ = uart.flush().await;

    let mut decoder = Decoder::new();
    let mut buf = [0u8; 128];
    loop {
        let n = match uart.read(&mut buf).await {
            Ok(n) if n > 0 => n,
            _ => continue,
        };
        for &b in &buf[..n] {
            if let Some(frame) = decoder.feed(b) {
                let ok = dispatch(frame).await;
                let _ = uart.write_all(&[if ok { ACK } else { NAK }]).await;
                let _ = uart.flush().await;
                LED_SIG.signal(if ok { LedEvent::Ok } else { LedEvent::Err });
            }
        }
    }
}

/// Returns true on ACK-worthy outcome, false to signal NAK to the host.
///
/// For per-frame report enqueues (Kbd/Mouse/Consumer) a full channel means
/// the target PC has stopped polling its USB IN endpoint and queued reports
/// have nowhere to drain. We surface that as NAK so the host doesn't
/// quietly lose keystrokes — see HOM-130. The README's "silently drop when
/// no target is present" remains accurate for the *try_send* outcome, but
/// the wire-level reply now honestly reflects whether the report landed.
///
/// Reset is the panic button for "release everything that's currently
/// held"; if the channel was full we drain it first so the all-zeros
/// report cannot be the one that's dropped.
async fn dispatch(frame: Frame) -> bool {
    match frame {
        Frame::Kbd { modifiers, keys } => KBD_CHAN
            .try_send(KeyboardReport {
                modifier: modifiers,
                reserved: 0,
                leds: 0,
                keycodes: keys,
            })
            .is_ok(),
        #[cfg(not(feature = "mister"))]
        Frame::Mouse { buttons, dx, dy, wheel } => MOUSE_CHAN
            .try_send(MouseReport {
                buttons,
                x: dx,
                y: dy,
                wheel,
                pan: 0,
            })
            .is_ok(),
        #[cfg(feature = "mister")]
        Frame::Mouse { .. } => false,
        #[cfg(not(feature = "mister"))]
        Frame::Consumer { usage } => CONSUMER_CHAN
            .try_send(MediaKeyboardReport { usage_id: usage })
            .is_ok(),
        #[cfg(feature = "mister")]
        Frame::Consumer { .. } => false,
        Frame::Delay { ms } => {
            let clamped = if ms > MAX_DELAY_MS {
                warn!("DELAY {} ms clamped to {} ms", ms, MAX_DELAY_MS);
                MAX_DELAY_MS
            } else {
                ms
            };
            Timer::after_millis(clamped as u64).await;
            true
        }
        Frame::Reset => {
            // Drain any pending stale reports so the zero-state payload
            // below cannot be the one dropped on a full channel.
            while KBD_CHAN.try_receive().is_ok() {}
            let kbd_ok = KBD_CHAN
                .try_send(KeyboardReport {
                    modifier: 0,
                    reserved: 0,
                    leds: 0,
                    keycodes: [0; 6],
                })
                .is_ok();
            #[cfg(not(feature = "mister"))]
            let aux_ok = {
                while MOUSE_CHAN.try_receive().is_ok() {}
                while CONSUMER_CHAN.try_receive().is_ok() {}
                MOUSE_CHAN
                    .try_send(MouseReport {
                        buttons: 0,
                        x: 0,
                        y: 0,
                        wheel: 0,
                        pan: 0,
                    })
                    .is_ok()
                    && CONSUMER_CHAN
                        .try_send(MediaKeyboardReport { usage_id: 0 })
                        .is_ok()
            };
            #[cfg(feature = "mister")]
            let aux_ok = true;
            kbd_ok && aux_ok
        }
    }
}

#[embassy_executor::task]
pub async fn hid_kbd_task(mut w: KbdWriter) {
    loop {
        let rpt = KBD_CHAN.receive().await;
        let _ = w.write_serialize(&rpt).await;
    }
}

#[cfg(not(feature = "mister"))]
#[embassy_executor::task]
pub async fn hid_mouse_task(mut w: MouseWriter) {
    loop {
        let rpt = MOUSE_CHAN.receive().await;
        let _ = w.write_serialize(&rpt).await;
    }
}

#[cfg(not(feature = "mister"))]
#[embassy_executor::task]
pub async fn hid_consumer_task(mut w: ConsumerWriter) {
    loop {
        let rpt = CONSUMER_CHAN.receive().await;
        let _ = w.write_serialize(&rpt).await;
    }
}
