use std::sync::{Condvar, Mutex};
use std::thread;
use std::time::{Duration, SystemTime, UNIX_EPOCH};

use anyhow::{bail, Context};
use embedded_svc::http::client::Client;
use esp_idf_svc::http::client::{Configuration, EspHttpConnection};
use esp_idf_svc::sntp::EspSntp;
use log::{info, warn};

use crate::config::HEALTHCHECK_URL;

const HEARTBEAT_INTERVAL: Duration = Duration::from_secs(60 * 60);
const REQUEST_TIMEOUT: Duration = Duration::from_secs(20);
const EARLIEST_VALID_UNIX_TIME: Duration = Duration::from_secs(1_577_836_800); // 2020-01-01
const THREAD_STACK_SIZE: usize = 16 * 1024;
static TIME_SYNC_LOCK: Mutex<()> = Mutex::new(());
static TIME_SYNCED: Condvar = Condvar::new();

pub fn start() -> anyhow::Result<()> {
    let sntp = EspSntp::new_with_callback(&Default::default(), |_| {
        let _guard = TIME_SYNC_LOCK
            .lock()
            .unwrap_or_else(|poisoned| poisoned.into_inner());
        TIME_SYNCED.notify_all();
    })
    .context("could not start SNTP")?;
    thread::Builder::new()
        .name("heartbeat".into())
        .stack_size(THREAD_STACK_SIZE)
        .spawn(move || run(sntp))?;

    Ok(())
}

fn run(_sntp: EspSntp<'static>) {
    wait_for_valid_time();

    loop {
        match send() {
            Ok(()) => info!("Hourly heartbeat accepted"),
            Err(error) => warn!("Hourly heartbeat failed: {error:#}"),
        }

        thread::sleep(HEARTBEAT_INTERVAL);
    }
}

pub(crate) fn wait_for_valid_time() {
    if system_time_is_valid() {
        return;
    }

    info!("Waiting for SNTP time synchronization before enabling HTTPS");
    let mut guard = TIME_SYNC_LOCK
        .lock()
        .unwrap_or_else(|poisoned| poisoned.into_inner());

    while !system_time_is_valid() {
        guard = TIME_SYNCED
            .wait(guard)
            .unwrap_or_else(|poisoned| poisoned.into_inner());
    }

    drop(guard);
    info!("System time synchronized; HTTPS certificate dates can be validated");
}

fn system_time_is_valid() -> bool {
    SystemTime::now()
        .duration_since(UNIX_EPOCH)
        .is_ok_and(|elapsed| elapsed >= EARLIEST_VALID_UNIX_TIME)
}

fn send() -> anyhow::Result<()> {
    let configuration = Configuration {
        timeout: Some(REQUEST_TIMEOUT),
        crt_bundle_attach: Some(esp_idf_svc::sys::esp_crt_bundle_attach),
        ..Default::default()
    };
    let connection =
        EspHttpConnection::new(&configuration).context("could not create HTTPS connection")?;
    let mut client = Client::wrap(connection);
    let request = client
        .post(HEALTHCHECK_URL, &[("content-length", "0")])
        .context("could not start heartbeat POST")?;
    let response = request
        .submit()
        .context("could not submit heartbeat POST")?;
    let status = response.status();

    if !(200..300).contains(&status) {
        bail!("heartbeat server returned HTTP {status}");
    }

    Ok(())
}

pub(crate) fn send_report_failure(message: &str) -> anyhow::Result<()> {
    let failure_url = format!("{}/fail", HEALTHCHECK_URL.trim_end_matches('/'));
    let content_length = message.len().to_string();
    let configuration = Configuration {
        timeout: Some(REQUEST_TIMEOUT),
        crt_bundle_attach: Some(esp_idf_svc::sys::esp_crt_bundle_attach),
        ..Default::default()
    };
    let connection = EspHttpConnection::new(&configuration)
        .context("could not create failure notification HTTPS connection")?;
    let mut client = Client::wrap(connection);
    let headers = [
        ("content-type", "text/plain; charset=utf-8"),
        ("content-length", content_length.as_str()),
    ];
    let mut request = client
        .post(&failure_url, &headers)
        .context("could not start failure notification POST")?;
    request
        .write(message.as_bytes())
        .context("could not write failure notification body")?;
    let response = request
        .submit()
        .context("could not submit failure notification POST")?;
    let status = response.status();

    if !(200..300).contains(&status) {
        bail!("failure notification server returned HTTP {status}");
    }

    Ok(())
}
