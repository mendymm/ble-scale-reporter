use std::sync::{
    atomic::{AtomicBool, Ordering},
    Arc,
};
use std::thread;
use std::time::Duration;

use esp_idf_svc::hal::gpio::{Gpio2, Output, PinDriver};

const BLINK_INTERVAL: Duration = Duration::from_millis(500);
const THREAD_STACK_SIZE: usize = 4 * 1024;

#[derive(Clone)]
pub struct StatusLight {
    healthy: Arc<AtomicBool>,
}

pub fn start(pin: Gpio2<'static>) -> anyhow::Result<StatusLight> {
    let healthy = Arc::new(AtomicBool::new(false));
    let thread_healthy = healthy.clone();
    let led = PinDriver::output(pin)?;

    thread::Builder::new()
        .name("status-light".into())
        .stack_size(THREAD_STACK_SIZE)
        .spawn(move || run(led, thread_healthy))?;

    Ok(StatusLight { healthy })
}

impl StatusLight {
    pub fn healthy(&self) {
        self.healthy.store(true, Ordering::Release);
    }
}

fn run(mut led: PinDriver<'static, Output>, healthy: Arc<AtomicBool>) {
    if led.set_low().is_err() {
        return;
    }

    loop {
        if healthy.load(Ordering::Acquire) {
            let _ = led.set_low();
            return;
        }

        if toggle(&mut led).is_err() {
            return;
        }

        thread::sleep(BLINK_INTERVAL);
    }
}

fn toggle(led: &mut PinDriver<'static, Output>) -> Result<(), esp_idf_svc::sys::EspError> {
    if led.is_set_high() {
        led.set_low()
    } else {
        led.set_high()
    }
}
