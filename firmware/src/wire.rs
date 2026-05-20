use embassy_rp::peripherals::USB;
use embassy_rp::uart::BufferedUart;
use embassy_rp::usb::Driver as UsbDriver;
use embassy_sync::blocking_mutex::raw::CriticalSectionRawMutex;
use embassy_sync::channel::Channel;
use embassy_time::Timer;
use embassy_usb::class::hid::HidWriter;
use embedded_io_async::{Read, Write};
use pico_keeb_protocol::binary::{Decoder, Frame, ACK, NAK};
use usbd_hid::descriptor::{KeyboardReport, MediaKeyboardReport, MouseReport};

use crate::led::{LedEvent, LED_SIG};

type UsbDriverT = UsbDriver<'static, USB>;

pub type KbdWriter = HidWriter<'static, UsbDriverT, 8>;
#[cfg(not(feature = "mister"))]
pub type MouseWriter = HidWriter<'static, UsbDriverT, 8>;
#[cfg(not(feature = "mister"))]
pub type ConsumerWriter = HidWriter<'static, UsbDriverT, 2>;

// HID reports are queued through bounded channels so the UART task can
// ACK each frame the instant it's decoded, without waiting for the USB
// host to poll the IN endpoint.
pub static KBD_CHAN: Channel<CriticalSectionRawMutex, KeyboardReport, 32> = Channel::new();
pub static MOUSE_CHAN: Channel<CriticalSectionRawMutex, MouseReport, 32> = Channel::new();
pub static CONSUMER_CHAN: Channel<CriticalSectionRawMutex, MediaKeyboardReport, 16> =
    Channel::new();

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
                let delivered = dispatch(frame).await;
                let reply = if delivered { ACK } else { NAK };
                let _ = uart.write_all(&[reply]).await;
                let _ = uart.flush().await;
                LED_SIG.signal(if delivered { LedEvent::Ok } else { LedEvent::Err });
            }
        }
    }
}

/// Enqueue the frame's HID report. Returns `false` if the destination
/// channel was full and the report was dropped — the caller turns that
/// into a `NAK` so the host can detect the silent loss. `Reset` always
/// returns `true` because it drains its targets first.
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
        Frame::Mouse { buttons, dx, dy, wheel } => MOUSE_CHAN
            .try_send(MouseReport {
                buttons,
                x: dx,
                y: dy,
                wheel,
                pan: 0,
            })
            .is_ok(),
        Frame::Consumer { usage } => CONSUMER_CHAN
            .try_send(MediaKeyboardReport { usage_id: usage })
            .is_ok(),
        Frame::Delay { ms } => {
            Timer::after_millis(ms as u64).await;
            true
        }
        Frame::Reset => {
            // Drain any pending reports so the all-zeros "release everything"
            // payload is what the HID writers see next, even if the channels
            // were full when Reset arrived.
            while KBD_CHAN.try_receive().is_ok() {}
            while MOUSE_CHAN.try_receive().is_ok() {}
            while CONSUMER_CHAN.try_receive().is_ok() {}
            let _ = KBD_CHAN.try_send(KeyboardReport {
                modifier: 0,
                reserved: 0,
                leds: 0,
                keycodes: [0; 6],
            });
            let _ = MOUSE_CHAN.try_send(MouseReport {
                buttons: 0,
                x: 0,
                y: 0,
                wheel: 0,
                pan: 0,
            });
            let _ = CONSUMER_CHAN.try_send(MediaKeyboardReport { usage_id: 0 });
            true
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
