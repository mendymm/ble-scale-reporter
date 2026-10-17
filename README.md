# BLE Scale Reporter

ESP32 firmware for reading a `2A26P-ESCS20MB2` scale over BLE. The goal is to automatically send weight measurements to a home server over WiFi.

## Setup

Create a `.env` file:

```sh
WIFI_SSID=...
WIFI_PASSWORD=...
HEALTHCHECK_URL=...
```

Build, flash, and monitor the ESP32:

```sh
cargo run --release
```

The last authoritative weight is stored in NVS and used as the scale's previous weight on the next measurement.
