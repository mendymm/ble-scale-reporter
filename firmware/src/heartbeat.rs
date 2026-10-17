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
const TIME_SYNC_RETRY_INTERVAL: Duration = Duration::from_secs(30);
const EARLIEST_VALID_UNIX_TIME: Duration = Duration::from_secs(1_577_836_800); // 2020-01-01
const THREAD_STACK_SIZE: usize = 16 * 1024;

pub fn start() -> anyhow::Result<()> {
    let sntp = EspSntp::new_default().context("could not start SNTP")?;
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

fn wait_for_valid_time() {
    info!("Waiting for SNTP time synchronization before enabling HTTPS");

    loop {
        let time_is_valid = SystemTime::now()
            .duration_since(UNIX_EPOCH)
            .is_ok_and(|elapsed| elapsed >= EARLIEST_VALID_UNIX_TIME);

        if time_is_valid {
            info!("System time synchronized; HTTPS certificate dates can be validated");
            return;
        }

        warn!("System time is not synchronized yet; heartbeat remains paused");
        thread::sleep(TIME_SYNC_RETRY_INTERVAL);
    }
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
