use std::sync::mpsc::{self, Receiver, RecvTimeoutError, Sender};
use std::thread;
use std::time::{Duration, Instant};

use log::{info, warn};

use crate::report::SampleReport;

const MEASUREMENT_WINDOW: Duration = Duration::from_secs(8);
const SESSION_GAP: Duration = Duration::from_secs(1);
const MAX_SAMPLES: usize = 50;
const THREAD_STACK_SIZE: usize = 8 * 1024;

pub(crate) enum MeasurementEvent {
    Sample(u16),
    SessionEnded,
}

pub(crate) fn start(reporter: Sender<SampleReport>) -> anyhow::Result<Sender<MeasurementEvent>> {
    let (sender, receiver) = mpsc::channel();
    thread::Builder::new()
        .name("weight-collector".into())
        .stack_size(THREAD_STACK_SIZE)
        .spawn(move || run(receiver, reporter))?;

    Ok(sender)
}

fn run(receiver: Receiver<MeasurementEvent>, reporter: Sender<SampleReport>) {
    while let Ok(event) = receiver.recv() {
        let MeasurementEvent::Sample(first_sample) = event else {
            continue;
        };

        let started = Instant::now();
        let mut samples = Vec::with_capacity(MAX_SAMPLES);
        samples.push(first_sample);

        let session_ended = loop {
            let remaining = MEASUREMENT_WINDOW.saturating_sub(started.elapsed());
            if remaining.is_zero() {
                break false;
            }

            match receiver.recv_timeout(remaining) {
                Ok(MeasurementEvent::Sample(sample)) => {
                    if samples.len() < MAX_SAMPLES {
                        samples.push(sample);
                    }
                }
                Ok(MeasurementEvent::SessionEnded) => break true,
                Err(RecvTimeoutError::Timeout) => break false,
                Err(RecvTimeoutError::Disconnected) => {
                    finish_measurement(samples, &reporter);
                    return;
                }
            }
        };

        if !finish_measurement(samples, &reporter) {
            return;
        }

        if !session_ended && !discard_until_session_boundary(&receiver) {
            return;
        }
    }
}

fn discard_until_session_boundary(receiver: &Receiver<MeasurementEvent>) -> bool {
    loop {
        match receiver.recv_timeout(SESSION_GAP) {
            Ok(MeasurementEvent::Sample(_)) => {}
            Ok(MeasurementEvent::SessionEnded) | Err(RecvTimeoutError::Timeout) => return true,
            Err(RecvTimeoutError::Disconnected) => return false,
        }
    }
}

fn finish_measurement(samples: Vec<u16>, reporter: &Sender<SampleReport>) -> bool {
    info!("Collected {} live weight samples", samples.len());

    if reporter.send(SampleReport { samples }).is_err() {
        warn!("Could not queue live weight samples");
        return false;
    }

    true
}
