# BLE Scale Reporter

ESP32 firmware for reading a `2A26P-ESCS20MB2` scale over BLE. The goal is to automatically send weight measurements to a home server over WiFi.

## Setup

Create `firmware/.env`:

```sh
cp firmware/.env.example firmware/.env
```

Edit the copied file with your WiFi credentials, health-check URL, and report URL. The report URL may use either `http://` or `https://`.

Build, flash, and monitor the ESP32:

```sh
cd firmware
cargo run --release
```

The last authoritative weight is stored in NVS and used as the scale's previous weight on the next measurement.
