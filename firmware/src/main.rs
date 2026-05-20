#![no_std]
#![no_main]

use defmt::info;
use embassy_executor::Spawner;
use embassy_rp::bind_interrupts;
use embassy_rp::peripherals::{PIO0, UART0, USB};
use embassy_rp::pio::{InterruptHandler as PioIrq, Pio};
use embassy_rp::pio_programs::ws2812::{PioWs2812, PioWs2812Program};
use embassy_rp::uart::{BufferedInterruptHandler, BufferedUart, Config as UartConfig};
use embassy_rp::usb::{Driver as UsbDriver, InterruptHandler as UsbIrq};
use embassy_usb::class::hid::{Config as HidConfig, HidWriter, State as HidState};
use embassy_usb::{Builder, Config as UsbConfig};
use static_cell::StaticCell;
use usbd_hid::descriptor::{KeyboardReport, SerializedDescriptor};
#[cfg(not(feature = "mister"))]
use usbd_hid::descriptor::{MediaKeyboardReport, MouseReport};

use {defmt_rtt as _, panic_probe as _};

mod led;
mod wire;

bind_interrupts!(struct Irqs {
    USBCTRL_IRQ => UsbIrq<USB>;
    UART0_IRQ => BufferedInterruptHandler<UART0>;
    PIO0_IRQ_0 => PioIrq<PIO0>;
});

type UsbDriverT = UsbDriver<'static, USB>;
type UsbDev = embassy_usb::UsbDevice<'static, UsbDriverT>;

// HID polling interval.
//   low-latency: 1 ms (desktop xHCI only)
//   mister:     16 ms (single interrupt pipe on dwc2 + hub)
//   default:     8 ms (compatible with most hosts)
#[cfg(feature = "low-latency")]
const HID_POLL_MS: u8 = 1;
#[cfg(all(feature = "mister", not(feature = "low-latency")))]
const HID_POLL_MS: u8 = 16;
#[cfg(not(any(feature = "low-latency", feature = "mister")))]
const HID_POLL_MS: u8 = 8;

#[embassy_executor::main]
async fn main(spawner: Spawner) {
    let p = embassy_rp::init(Default::default());
    info!("pico-keeb boot");

    // ---- USB stack ----
    static CONFIG_DESC: StaticCell<[u8; 256]> = StaticCell::new();
    static BOS_DESC: StaticCell<[u8; 256]> = StaticCell::new();
    static MSOS_DESC: StaticCell<[u8; 64]> = StaticCell::new();
    static CONTROL_BUF: StaticCell<[u8; 64]> = StaticCell::new();
    static KBD_STATE: StaticCell<HidState> = StaticCell::new();
    #[cfg(not(feature = "mister"))]
    static MOUSE_STATE: StaticCell<HidState> = StaticCell::new();
    #[cfg(not(feature = "mister"))]
    static CONSUMER_STATE: StaticCell<HidState> = StaticCell::new();

    let usb_driver = UsbDriver::new(p.USB, Irqs);

    let mut usb_cfg = UsbConfig::new(0x16c0, 0x27db);
    usb_cfg.manufacturer = Some("pico-keeb");
    usb_cfg.product = Some("pico-keeb");
    usb_cfg.serial_number = Some("0001");
    // 250 mA gives the WS2812 status LED's peak-current pulses headroom
    // so we don't trip a hub port's over-current detect.
    usb_cfg.max_power = 250;
    usb_cfg.max_packet_size_0 = 64;

    let mut builder = Builder::new(
        usb_driver,
        usb_cfg,
        CONFIG_DESC.init([0; 256]),
        BOS_DESC.init([0; 256]),
        MSOS_DESC.init([0; 64]),
        CONTROL_BUF.init([0; 64]),
    );

    // Endpoint max-packet matches the actual HID report size so dwc2's
    // split-transaction scheduler only reserves the bandwidth it uses.
    let kbd_writer = HidWriter::<_, 8>::new(
        &mut builder,
        KBD_STATE.init(HidState::new()),
        HidConfig {
            report_descriptor: KeyboardReport::desc(),
            request_handler: None,
            poll_ms: HID_POLL_MS,
            max_packet_size: 8,
        },
    );

    #[cfg(not(feature = "mister"))]
    let mouse_writer = HidWriter::<_, 8>::new(
        &mut builder,
        MOUSE_STATE.init(HidState::new()),
        HidConfig {
            report_descriptor: MouseReport::desc(),
            request_handler: None,
            poll_ms: HID_POLL_MS,
            max_packet_size: 8,
        },
    );

    #[cfg(not(feature = "mister"))]
    let consumer_writer = HidWriter::<_, 2>::new(
        &mut builder,
        CONSUMER_STATE.init(HidState::new()),
        HidConfig {
            report_descriptor: MediaKeyboardReport::desc(),
            request_handler: None,
            poll_ms: HID_POLL_MS,
            max_packet_size: 2,
        },
    );

    let usb = builder.build();
    spawner.must_spawn(usb_run(usb));
    spawner.must_spawn(wire::hid_kbd_task(kbd_writer));
    #[cfg(not(feature = "mister"))]
    spawner.must_spawn(wire::hid_mouse_task(mouse_writer));
    #[cfg(not(feature = "mister"))]
    spawner.must_spawn(wire::hid_consumer_task(consumer_writer));

    // ---- UART0 on GP0 (TX) / GP1 (RX) @ 921600 ----
    static UART_RX_BUF: StaticCell<[u8; 1024]> = StaticCell::new();
    static UART_TX_BUF: StaticCell<[u8; 64]> = StaticCell::new();
    let mut uart_cfg = UartConfig::default();
    uart_cfg.baudrate = 921_600;
    let uart = BufferedUart::new(
        p.UART0,
        p.PIN_0,
        p.PIN_1,
        Irqs,
        UART_TX_BUF.init([0; 64]),
        UART_RX_BUF.init([0; 1024]),
        uart_cfg,
    );
    spawner.must_spawn(wire::uart_task(uart));

    // ---- WS2812 status LED on GP16 ----
    let Pio {
        mut common, sm0, ..
    } = Pio::new(p.PIO0, Irqs);
    let ws_program = PioWs2812Program::new(&mut common);
    let ws2812 = PioWs2812::new(&mut common, sm0, p.DMA_CH0, p.PIN_16, &ws_program);
    spawner.must_spawn(led::led_task(ws2812));
}

#[embassy_executor::task]
async fn usb_run(mut dev: UsbDev) -> ! {
    dev.run().await
}
