use defmt::warn;
use embassy_rp::peripherals::USB;
use embassy_rp::uart::BufferedUart;
use embassy_rp::usb::Driver as UsbDriver;
use embassy_sync::blocking_mutex::raw::CriticalSectionRawMutex;
use embassy_sync::channel::Channel;
use embassy_time::Timer;
use embassy_usb::class::hid::HidWriter;
use embedded_io_async::{Read, Write};
use pico_keeb_protocol::binary::{Decoder, Frame, ACK};
use usbd_hid::descriptor::{KeyboardReport, MediaKeyboardReport, MouseReport};

use crate::led::{LedEvent, LED_SIG};

// Cap on a single Delay frame. The UART RX buffer is 1024 B and at
// 921600 baud fills in ~83 ms, so a long inline delay would overflow
// the FIFO and desync the decoder. 5 s covers any realistic macro
// pause; longer pauses must be chunked by the host.
const MAX_DELAY_MS: u32 = 5_000;

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
                dispatch(frame).await;
                let _ = uart.write_all(&[ACK]).await;
                let _ = uart.flush().await;
                LED_SIG.signal(LedEvent::Ok);
            }
        }
    }
}

async fn dispatch(frame: Frame) {
    // try_send + drop-on-full: if the HID target isn't polling its IN
    // endpoint, the channel fills once and all subsequent reports are
    // dropped. That keeps the ACK path responsive and prevents stale
    // reports from firing when a target eventually reconnects.
    match frame {
        Frame::Kbd { modifiers, keys } => {
            let _ = KBD_CHAN.try_send(KeyboardReport {
                modifier: modifiers,
                reserved: 0,
                leds: 0,
                keycodes: keys,
            });
        }
        Frame::Mouse { buttons, dx, dy, wheel } => {
            let _ = MOUSE_CHAN.try_send(MouseReport {
                buttons,
                x: dx,
                y: dy,
                wheel,
                pan: 0,
            });
        }
        Frame::Consumer { usage } => {
            let _ = CONSUMER_CHAN.try_send(MediaKeyboardReport { usage_id: usage });
        }
        Frame::Delay { ms } => {
            let clamped = if ms > MAX_DELAY_MS {
                warn!("DELAY {} ms clamped to {} ms", ms, MAX_DELAY_MS);
                MAX_DELAY_MS
            } else {
                ms
            };
            Timer::after_millis(clamped as u64).await;
        }
        Frame::Reset => {
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
