mod base64;
mod config;
mod heartbeat;
mod protocol;
mod report;
mod scale;
mod status;
mod wifi;

fn main() -> anyhow::Result<()> {
    esp_idf_svc::sys::link_patches();
    esp_idf_svc::log::init(if cfg!(debug_assertions) {
        log::LevelFilter::Debug
    } else {
        log::LevelFilter::Info
    });

    let peripherals = esp_idf_svc::hal::peripherals::Peripherals::take()?;
    let status_light = status::start(peripherals.pins.gpio2)?;
    let (wifi_modem, bluetooth_modem) = peripherals.modem.split();
    let nvs = esp_idf_svc::nvs::EspDefaultNvsPartition::take()?;

    wifi::start(wifi_modem, nvs.clone())?;
    heartbeat::start()?;
    let reporter = report::start()?;
    scale::run(bluetooth_modem, nvs, reporter, status_light)
}
