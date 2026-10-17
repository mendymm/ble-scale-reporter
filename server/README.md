# BLE Scale Server

Linux server that receives measurements from the BLE scale firmware, stores
each report as JSON and in SQLite, and serves an embedded weight-history UI.

## Run

Pass the TLS private key, certificate chain, and persistent data directory on
the command line. The server listens for HTTPS connections on `0.0.0.0:443`.

```sh
cd server
./target/release/ble-scale-server key.pem cert.pem /var/lib/ble-scale
```

Run `ble-scale-server --help` to print the command usage.

The data layout is:

```text
$DATA_DIR/
├── reports/
│   └── <unix-timestamp>.json
└── reports.sqlite
```

## API

`POST /api/report` accepts a Unix timestamp in seconds, a calculated weight in
hundredths of a kilogram, and the raw live-weight samples used by the firmware:

```json
{"timestamp": 1760616000, "weight": 8085, "samples": [8030, 8072, 8084, 8085]}
```

`GET /api/reports` returns all measurements in chronological order. `GET /`
serves the UI. Samples are stored as JSON text in the `samples` SQLite column;
the UI currently ignores them.

## Build a static Linux executable

SQLite is compiled into the binary by rusqlite's `bundled` feature. Build for a
musl target to statically link the remaining C runtime:

```sh
cd server
cargo build --release --target x86_64-unknown-linux-musl
```

Binding port 443 requires root or `CAP_NET_BIND_SERVICE`. To grant that
capability to the built binary:

```sh
sudo setcap cap_net_bind_service=+ep \
  target/x86_64-unknown-linux-musl/release/ble-scale-server
```

For 64-bit ARM Linux, use `aarch64-unknown-linux-musl` with an appropriate
cross-linker.
