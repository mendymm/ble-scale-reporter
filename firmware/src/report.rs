use std::sync::mpsc::{self, Receiver, Sender};
use std::thread;
use std::time::{Duration, SystemTime, UNIX_EPOCH};

use anyhow::{bail, Context};
use embedded_svc::http::client::Client;
use esp_idf_svc::http::client::{Configuration, EspHttpConnection};
use log::{info, warn};

use crate::config::REPORT_URL;
use crate::heartbeat::wait_for_valid_time;
use crate::protocol::HundredthsKg;

const REQUEST_TIMEOUT: Duration = Duration::from_secs(20);
const THREAD_STACK_SIZE: usize = 16 * 1024;

pub fn start() -> anyhow::Result<Sender<HundredthsKg>> {
    let (sender, receiver) = mpsc::channel();
    thread::Builder::new()
        .name("weight-report".into())
        .stack_size(THREAD_STACK_SIZE)
        .spawn(move || run(receiver))?;

    Ok(sender)
}

fn run(receiver: Receiver<HundredthsKg>) {
    for weight in receiver {
        wait_for_valid_time();

        match send(weight) {
            Ok(()) => info!("Authoritative weight report accepted: {weight}"),
            Err(error) => warn!("Authoritative weight report failed: {error:#}"),
        }
    }
}

fn send(weight: HundredthsKg) -> anyhow::Result<()> {
    let timestamp = SystemTime::now()
        .duration_since(UNIX_EPOCH)
        .context("system time is before Unix epoch")?
        .as_secs();
    let body = format!(
        r#"{{"timestamp":{timestamp},"weight":{}}}"#,
        weight.hundredths()
    );
    let content_length = body.len().to_string();
    let configuration = Configuration {
        timeout: Some(REQUEST_TIMEOUT),
        crt_bundle_attach: Some(esp_idf_svc::sys::esp_crt_bundle_attach),
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
