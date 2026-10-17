# BLE Scale Reporter

The goal is to track my weight by standing on the scale—and nothing else. An
ESP32 reads my `2A26P-ESCS20MB2` scale over BLE, calculates the weight, and
sends it over WiFi to a small home server with a weight-history UI.

The main requirement is that weight data does not depend on a cloud service.
The firmware, API, storage, and UI are all self-hosted.

## How it works

The firmware collects up to 50 live readings over at most eight seconds. It
finds the largest cluster spanning no more than one kilogram and uses that
cluster's median as the final weight. The calculated weight and raw readings
are sent to the server.

The server stores every report as both JSON and SQLite data. The UI hides
sessions with fewer than five samples and suppresses duplicate measurements
within 30 seconds. Raw reports remain stored for future analysis.

## Certificates

The root CA used by the firmware and the TLS certificates used by my server are
certificates from my own private CA. I use them for convenience on my network;
they are not shared project credentials or required infrastructure.

Use your own CA and server certificate. Place its root certificate at
`firmware/root-ca.cert.pem`; it is ignored by Git and embedded into the firmware.
Pass the matching server key and certificate to the server at startup.

## Firmware

Create the firmware configuration:

```sh
cp firmware/.env.example firmware/.env
```

Set `WIFI_SSID`, `WIFI_PASSWORD`, `HEALTHCHECK_URL`, and `REPORT_URL`, then add
your root CA certificate and build or flash the ESP32:

```sh
cd firmware
cargo run --release
```

The health-check URL is configurable; it does not need to use a hosted service.

## Server

The Linux server listens on HTTPS port 443. Pass its private key, certificate
chain, and data directory:

```sh
cd server
./target/release/ble-scale-server key.pem cert.pem /var/lib/ble-scale
```

Run `ble-scale-server --help` for usage. Binding port 443 requires root or
`CAP_NET_BIND_SERVICE`.

Data is stored as:

```text
$DATA_DIR/
├── reports/
│   └── <unix-timestamp>.json
└── reports.sqlite
```

## API

`POST /api/report` accepts a Unix timestamp, a weight in hundredths of a
kilogram, and the raw readings:

```json
{"timestamp":1760616000,"weight":8085,"samples":[8030,8072,8084,8085,8085]}
```

`GET /api/reports` supplies measurements to the UI. `GET /` serves the UI.

## Static server build

SQLite is bundled. Build a static Linux executable with musl:

```sh
cd server
cargo build --release --target x86_64-unknown-linux-musl
```

For 64-bit ARM, use `aarch64-unknown-linux-musl` and its appropriate linker.
