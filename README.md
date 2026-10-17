# BLE Scale Reporter

The goal is to track my weight by standing on the scale—and nothing else. An
ESP32 reads my `2A26P-ESCS20MB2` scale over BLE, calculates the weight, and
sends it over WiFi to a small home server with a weight-history UI.

The main requirement is that weight data does not depend on a cloud service.
The firmware, API, storage, and UI are all self-hosted.

## How it works

The firmware collects up to 50 live readings over at most eight seconds and
sends those raw readings to the server.

The JSON report files are the source of truth. At startup, the server rebuilds
SQLite from them and recalculates every weight. This makes the algorithm
tunable without losing the original data.

Readings below 35 kg are ignored. The server finds the largest cluster spanning
no more than one kilogram and uses its median. It rejects a result that differs
by at least 10 kg from an accepted measurement in the previous 60 days. The UI
also hides sessions with fewer than five samples and duplicate measurements
within 30 seconds.

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

`POST /api/report` accepts a Unix timestamp and raw readings in hundredths of a
kilogram:

```json
{"timestamp":1760616000,"samples":[8030,8072,8084,8085,8085]}
```

`GET /api/reports` supplies measurements to the UI. `GET /` serves the UI.

## Static server build

SQLite is bundled. Build a static Linux executable with musl:

```sh
cd server
cargo build --release --target x86_64-unknown-linux-musl
```

For 64-bit ARM, use `aarch64-unknown-linux-musl` and its appropriate linker.
