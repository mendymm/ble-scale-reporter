use std::{
    env,
    error::Error,
    ffi::OsString,
    io,
    net::SocketAddr,
    path::PathBuf,
    time::Duration,
};

use axum_server::{Handle, tls_rustls::RustlsConfig};
use ble_scale_server::{Storage, router};
use tokio::signal;
use tracing::{Level, info};

const HELP: &str = "\
Usage: ble-scale-server <key.pem> <cert.pem> <data-dir>

Arguments:
  <key.pem>   PEM-encoded TLS private key
  <cert.pem>  PEM-encoded TLS certificate chain
  <data-dir>  Directory for reports and reports.sqlite

Options:
  -h, --help  Print help

The HTTPS server listens on 0.0.0.0:443.
";

enum Command {
    Help,
    Run {
        key_path: PathBuf,
        certificate_path: PathBuf,
        data_dir: PathBuf,
    },
}

#[tokio::main]
async fn main() -> Result<(), Box<dyn Error>> {
    let Command::Run {
        key_path,
        certificate_path,
        data_dir,
    } = parse_command()?
    else {
        print!("{HELP}");
        return Ok(());
    };

    tracing_subscriber::fmt()
        .with_max_level(Level::INFO)
        .with_target(false)
        .init();

    rustls::crypto::ring::default_provider()
        .install_default()
        .map_err(|_| io::Error::other("could not install the rustls crypto provider"))?;

    let storage = Storage::open(&data_dir)?;
    let tls_config = RustlsConfig::from_pem_file(&certificate_path, &key_path).await?;
    let address = SocketAddr::from(([0, 0, 0, 0], 443));
    let handle = Handle::new();
    let shutdown_handle = handle.clone();

    tokio::spawn(async move {
        shutdown_signal().await;
        shutdown_handle.graceful_shutdown(Some(Duration::from_secs(10)));
    });

    info!(address = %address, data_dir = %data_dir.display(), "HTTPS server listening");

    axum_server::bind_rustls(address, tls_config)
        .handle(handle)
        .serve(router(storage).into_make_service())
        .await?;

    Ok(())
}

fn parse_command() -> Result<Command, io::Error> {
    let arguments = env::args_os().skip(1).collect::<Vec<_>>();

    if matches!(arguments.as_slice(), [argument] if is_help(argument)) {
        return Ok(Command::Help);
    }

    let [key_path, certificate_path, data_dir] = arguments.as_slice() else {
        return Err(io::Error::new(
            io::ErrorKind::InvalidInput,
            format!("expected a key, certificate, and data directory\n\n{HELP}"),
        ));
    };

    Ok(Command::Run {
        key_path: PathBuf::from(key_path),
        certificate_path: PathBuf::from(certificate_path),
        data_dir: PathBuf::from(data_dir),
    })
}

fn is_help(argument: &OsString) -> bool {
    argument == "-h" || argument == "--help"
}

async fn shutdown_signal() {
    let ctrl_c = async {
        signal::ctrl_c()
            .await
            .expect("failed to install Ctrl-C handler");
    };

    let terminate = async {
        signal::unix::signal(signal::unix::SignalKind::terminate())
            .expect("failed to install SIGTERM handler")
            .recv()
            .await;
    };

    tokio::select! {
        () = ctrl_c => {}
        () = terminate => {}
    }

    info!("shutdown signal received");
}
