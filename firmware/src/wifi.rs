use std::thread;
use std::time::Duration;

use anyhow::anyhow;
use esp_idf_svc::eventloop::EspSystemEventLoop;
use esp_idf_svc::hal::modem::WifiModem;
use esp_idf_svc::nvs::EspDefaultNvsPartition;
use esp_idf_svc::wifi::{AuthMethod, BlockingWifi, ClientConfiguration, Configuration, EspWifi};
use log::{info, warn};

use crate::config::{WIFI_PASSWORD, WIFI_SSID};

pub fn start(wifi_modem: WifiModem<'static>, nvs: EspDefaultNvsPartition) -> anyhow::Result<()> {
    let system_event_loop = EspSystemEventLoop::take()?;
    let wifi = EspWifi::new(wifi_modem, system_event_loop.clone(), Some(nvs))?;
    let mut wifi = BlockingWifi::wrap(wifi, system_event_loop)?;

    let configuration = Configuration::Client(ClientConfiguration {
        ssid: WIFI_SSID
            .try_into()
            .map_err(|_| anyhow!("WIFI_SSID is longer than 32 bytes"))?,
        password: WIFI_PASSWORD
            .try_into()
            .map_err(|_| anyhow!("WIFI_PASSWORD is longer than 64 bytes"))?,
        auth_method: if WIFI_PASSWORD.is_empty() {
            AuthMethod::None
        } else {
            AuthMethod::WPA2Personal
        },
        ..Default::default()
    });

    wifi.set_configuration(&configuration)?;
    wifi.start()?;
    wifi.connect()?;
    wifi.wait_netif_up()?;

    let ip = wifi.wifi().sta_netif().get_ip_info()?;
    info!("Wi-Fi connected to {WIFI_SSID}; IP address {}", ip.ip);

    thread::Builder::new()
        .name("wifi-supervisor".into())
        .spawn(move || supervise_connection(wifi))?;

    Ok(())
}

fn supervise_connection(mut wifi: BlockingWifi<EspWifi<'static>>) {
    loop {
        thread::sleep(Duration::from_secs(5));

        match wifi.is_connected() {
            Ok(true) => continue,
            Ok(false) => warn!("Wi-Fi disconnected; reconnecting"),
            Err(error) => {
                warn!("Could not query Wi-Fi connection state: {error}; reconnecting")
            }
        }

        if let Err(error) = wifi.connect().and_then(|()| wifi.wait_netif_up()) {
            warn!("Wi-Fi reconnect failed: {error}");
        } else {
            match wifi.wifi().sta_netif().get_ip_info() {
                Ok(ip) => info!("Wi-Fi reconnected; IP address {}", ip.ip),
                Err(error) => info!("Wi-Fi reconnected; could not read IP address: {error}"),
            }
        }
    }
}
