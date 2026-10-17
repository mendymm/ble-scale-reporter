use std::sync::mpsc::{self, Receiver, Sender};
use std::thread;
use std::time::{Duration, SystemTime, UNIX_EPOCH};

use anyhow::{bail, Context};
use embedded_svc::http::client::Client;
use esp_idf_svc::http::client::{Configuration, EspHttpConnection};
use esp_idf_svc::tls::X509;
use log::{info, warn};

use crate::config::REPORT_URL;
use crate::heartbeat::{send_report_failure, wait_for_valid_time};
use crate::protocol::HundredthsKg;

const REQUEST_TIMEOUT: Duration = Duration::from_secs(20);
const THREAD_STACK_SIZE: usize = 16 * 1024;
const REPORT_ROOT_CA: &[u8] = include_bytes!(concat!(env!("OUT_DIR"), "/report-root-ca.cert.pem"));

pub(crate) struct WeightReport {
    pub weight: u16,
    pub samples: Vec<u16>,
}

pub(crate) fn start() -> anyhow::Result<Sender<WeightReport>> {
    let (sender, receiver) = mpsc::channel();
    thread::Builder::new()
        .name("weight-report".into())
        .stack_size(THREAD_STACK_SIZE)
        .spawn(move || run(receiver))?;

    Ok(sender)
}

fn run(receiver: Receiver<WeightReport>) {
    for report in receiver {
        wait_for_valid_time();

        match send(&report) {
            Ok(()) => info!(
                "Calculated weight report accepted: {}",
                HundredthsKg::new(u32::from(report.weight))
            ),
            Err(error) => {
                warn!("Calculated weight report failed: {error:#}");
                let message = format!("Weight report failed: {error}");
                if let Err(notification_error) = send_report_failure(&message) {
                    warn!("Could not send report failure notification: {notification_error:#}");
                }
            }
        }
    }
}

fn send(report: &WeightReport) -> anyhow::Result<()> {
    let timestamp = SystemTime::now()
        .duration_since(UNIX_EPOCH)
        .context("system time is before Unix epoch")?
        .as_secs();
    let samples = report
        .samples
        .iter()
        .map(u16::to_string)
        .collect::<Vec<_>>()
        .join(",");
    let body = format!(
        r#"{{"timestamp":{timestamp},"weight":{},"samples":[{samples}]}}"#,
        report.weight
    );
    let content_length = body.len().to_string();
    let configuration = Configuration {
        timeout: Some(REQUEST_TIMEOUT),
        server_certificate: Some(X509::pem_until_nul(REPORT_ROOT_CA)),
        ..Default::default()
    };
    let connection = EspHttpConnection::new(&configuration)
        .context("could not create report HTTP connection")?;
    let mut client = Client::wrap(connection);

    let headers = [
        ("content-type", "application/json"),
        ("content-length", content_length.as_str()),
    ];
    let mut request = client
        .post(REPORT_URL, &headers)
        .context("could not start weight report POST")?;
    request
        .write(body.as_bytes())
        .context("could not write weight report body")?;
    let response = request
        .submit()
        .context("could not submit weight report POST")?;
    let status = response.status();

    if !(200..300).contains(&status) {
        bail!("report server returned HTTP {status}");
    }

    Ok(())
}
