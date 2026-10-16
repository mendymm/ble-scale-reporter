use std::io::Write;
use std::net::{TcpListener, TcpStream};
use std::thread;
use std::time::Duration;

use anyhow::{anyhow, Context};
use esp_idf_svc::eventloop::EspSystemEventLoop;
use esp_idf_svc::hal::modem::WifiModem;
use esp_idf_svc::nvs::EspDefaultNvsPartition;
use esp_idf_svc::wifi::{AuthMethod, BlockingWifi, ClientConfiguration, Configuration, EspWifi};
use log::{error, info, warn};

const WIFI_SSID: Option<&str> = option_env!("WIFI_SSID");
const WIFI_PASSWORD: Option<&str> = option_env!("WIFI_PASSWORD");
const TCP_ADDRESS: &str = "0.0.0.0:6969";
const RESPONSE: &[u8] = b"hello, bye";

pub fn start(wifi_modem: WifiModem<'static>, nvs: EspDefaultNvsPartition) -> anyhow::Result<()> {
    let ssid = WIFI_SSID.context(
        "WIFI_SSID was not set when the firmware was built; rebuild with WIFI_SSID and WIFI_PASSWORD",
    )?;
    let password = WIFI_PASSWORD.context(
        "WIFI_PASSWORD was not set when the firmware was built; rebuild with WIFI_SSID and WIFI_PASSWORD",
    )?;
    let system_event_loop = EspSystemEventLoop::take()?;
    let wifi = EspWifi::new(wifi_modem, system_event_loop.clone(), Some(nvs))?;
    let mut wifi = BlockingWifi::wrap(wifi, system_event_loop)?;

    let configuration = Configuration::Client(ClientConfiguration {
        ssid: ssid
            .try_into()
            .map_err(|_| anyhow!("WIFI_SSID is longer than 32 bytes"))?,
        password: password
            .try_into()
            .map_err(|_| anyhow!("WIFI_PASSWORD is longer than 64 bytes"))?,
        auth_method: if password.is_empty() {
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
    info!("Wi-Fi connected to {ssid}; IP address {}", ip.ip);

    start_tcp_server()?;
    thread::Builder::new()
        .name("wifi-supervisor".into())
        .spawn(move || supervise_connection(wifi))?;

    Ok(())
}

fn start_tcp_server() -> anyhow::Result<()> {
    let listener = TcpListener::bind(TCP_ADDRESS)
        .with_context(|| format!("failed to bind TCP server to {TCP_ADDRESS}"))?;

    thread::Builder::new()
        .name("tcp-6969".into())
        .spawn(move || {
            info!("TCP server listening on {TCP_ADDRESS}");
            for connection in listener.incoming() {
                match connection {
                    Ok(stream) => handle_connection(stream),
                    Err(error) => warn!("TCP accept failed: {error}"),
                }
            }
            error!("TCP server stopped unexpectedly");
        })?;

    Ok(())
}

fn handle_connection(mut stream: TcpStream) {
    match stream.peer_addr() {
        Ok(peer) => info!("TCP connection from {peer}"),
        Err(error) => warn!("Could not identify TCP peer: {error}"),
    }

    if let Err(error) = stream.write_all(RESPONSE) {
        warn!("Could not send TCP response: {error}");
    }
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
