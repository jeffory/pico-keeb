use embassy_rp::peripherals::PIO0;
use embassy_rp::pio_programs::ws2812::PioWs2812;
use embassy_sync::blocking_mutex::raw::CriticalSectionRawMutex;
use embassy_sync::signal::Signal;
use embassy_time::Timer;
use smart_leds::RGB8;

#[derive(Clone, Copy, Debug)]
#[allow(dead_code)]
pub enum LedEvent {
    Ok,
    Err,
}

pub static LED_SIG: Signal<CriticalSectionRawMutex, LedEvent> = Signal::new();

pub type Ws2812T = PioWs2812<'static, PIO0, 0, 1>;

#[embassy_executor::task]
pub async fn led_task(mut ws: Ws2812T) {
    let idle = RGB8 { r: 0, g: 0, b: 20 };
    let ok = RGB8 { r: 0, g: 60, b: 0 };
    let err = RGB8 { r: 60, g: 0, b: 0 };
    ws.write(&[idle]).await;
    loop {
        let ev = LED_SIG.wait().await;
        match ev {
            LedEvent::Ok => {
                ws.write(&[ok]).await;
                Timer::after_millis(80).await;
                ws.write(&[idle]).await;
            }
            LedEvent::Err => {
                ws.write(&[err]).await;
                Timer::after_millis(150).await;
                ws.write(&[idle]).await;
            }
        }
    }
}
