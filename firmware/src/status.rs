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
    ready: Arc<AtomicBool>,
}

pub fn start(pin: Gpio2<'static>) -> anyhow::Result<StatusLight> {
    let ready = Arc::new(AtomicBool::new(false));
    let thread_ready = ready.clone();
    let led = PinDriver::output(pin)?;

    thread::Builder::new()
        .name("status-light".into())
        .stack_size(THREAD_STACK_SIZE)
        .spawn(move || run(led, thread_ready))?;

    Ok(StatusLight { ready })
}

impl StatusLight {
    pub fn ready(&self) {
        self.ready.store(true, Ordering::Release);
    }
}

fn run(mut led: PinDriver<'static, Output>, ready: Arc<AtomicBool>) {
    let mut on = false;

    loop {
        if ready.load(Ordering::Acquire) {
            let _ = led.set_high();
            return;
        }

        on = !on;
        let result = if on { led.set_high() } else { led.set_low() };
        if result.is_err() {
            return;
        }

        thread::sleep(BLINK_INTERVAL);
    }
}
