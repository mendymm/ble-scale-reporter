use std::sync::mpsc::{self, Receiver, RecvTimeoutError, Sender};
use std::thread;
use std::time::{Duration, Instant};

use log::{info, warn};

use crate::protocol::HundredthsKg;
use crate::report::WeightReport;

const MEASUREMENT_WINDOW: Duration = Duration::from_secs(8);
const SESSION_GAP: Duration = Duration::from_secs(1);
const MAX_STABLE_SPAN: u16 = 100;
const MAX_SAMPLES: usize = 50;
const THREAD_STACK_SIZE: usize = 8 * 1024;

pub(crate) enum MeasurementEvent {
    Sample(u16),
    SessionEnded,
}

pub(crate) fn start(reporter: Sender<WeightReport>) -> anyhow::Result<Sender<MeasurementEvent>> {
    let (sender, receiver) = mpsc::channel();
    thread::Builder::new()
        .name("weight-calculator".into())
        .stack_size(THREAD_STACK_SIZE)
        .spawn(move || run(receiver, reporter))?;

    Ok(sender)
}

fn run(receiver: Receiver<MeasurementEvent>, reporter: Sender<WeightReport>) {
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

fn finish_measurement(samples: Vec<u16>, reporter: &Sender<WeightReport>) -> bool {
    let Some(weight) = calculate_weight(&samples) else {
        return true;
    };

    info!(
        "Calculated weight {} from {} live samples",
        HundredthsKg::new(u32::from(weight)),
        samples.len()
    );

    if reporter.send(WeightReport { weight, samples }).is_err() {
        warn!("Could not queue calculated weight report");
        return false;
    }

    true
}

fn calculate_weight(samples: &[u16]) -> Option<u16> {
    if samples.is_empty() {
        return None;
    }

    // A real measurement produces a dense band of nearby readings, while
    // stepping on and off produces a sparse ramp. Find the largest group with
    // no more than 1 kg between its lightest and heaviest values.
    let mut sorted = samples.to_vec();
    sorted.sort_unstable();

    let mut window_start = 0;
    let mut best_start = 0;
    let mut best_end = 1;

    for window_end in 0..sorted.len() {
        while sorted[window_end] - sorted[window_start] > MAX_STABLE_SPAN {
            window_start += 1;
        }

        let window_len = window_end + 1 - window_start;
        let best_len = best_end - best_start;
        let window_span = sorted[window_end] - sorted[window_start];
        let best_span = sorted[best_end - 1] - sorted[best_start];

        let equally_dense_but_better = window_len == best_len
            && (window_span < best_span
                || (window_span == best_span
                    && sorted[window_end] > sorted[best_end - 1]));

        if window_len > best_len || equally_dense_but_better {
            best_start = window_start;
            best_end = window_end + 1;
        }
    }

    let stable = &sorted[best_start..best_end];
    let middle = stable.len() / 2;

    if stable.len() % 2 == 0 {
        let sum = u32::from(stable[middle - 1]) + u32::from(stable[middle]);
        Some(((sum + 1) / 2) as u16)
    } else {
        Some(stable[middle])
    }
}

#[cfg(test)]
mod tests {
    use super::calculate_weight;

    #[test]
    fn uses_the_median_of_the_largest_stable_cluster() {
        let samples = [
            8_095, 8_030, 8_035, 8_085, 8_075, 8_070, 8_075, 8_035, 8_030, 8_085, 8_070,
            8_055, 8_060, 8_065, 8_055, 8_060, 7_565, 5_465, 335, 130, 50, 0,
        ];

        assert_eq!(calculate_weight(&samples), Some(8_063));
    }

    #[test]
    fn accepts_a_single_sample() {
        assert_eq!(calculate_weight(&[8_085]), Some(8_085));
    }

    #[test]
    fn prefers_the_heavier_cluster_when_scores_are_tied() {
        assert_eq!(calculate_weight(&[0, 0, 8_085, 8_085]), Some(8_085));
    }
}
