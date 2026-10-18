use std::{error::Error, io, net::SocketAddr, path::PathBuf, time::Duration};

use axum_server::{Handle, tls_rustls::RustlsConfig};
use ble_scale_server::{Storage, router};
use tokio::signal;
use tracing::{Level, info};

const HELP: &str = "\
Usage: ble-scale-server --data-dir <path> [options]

Options:
      --data-dir <path>  Directory for reports and reports.sqlite
      --port <port>      Port to listen on (default: 3000)
      --cert <path>      PEM-encoded TLS certificate chain
      --key <path>       PEM-encoded TLS private key
  -h, --help             Print help

The server uses HTTPS only when --port is 443 and both --cert and --key are set.
";

#[derive(Debug, PartialEq, Eq)]
enum Command {
    Help,
    RunHttps {
        port: u16,
        certificate_path: PathBuf,
        key_path: PathBuf,
        data_dir: PathBuf,
    },
    RunHttp {
        port: u16,
        data_dir: PathBuf,
    },
}

#[tokio::main]
async fn main() -> Result<(), Box<dyn Error>> {
    let command = parse_command()?;

    tracing_subscriber::fmt()
        .with_max_level(Level::INFO)
        .with_target(false)
        .init();

    let (port, data_dir) = match &command {
        Command::Help => {
            print!("{HELP}");
            return Ok(());
        }
        Command::RunHttps { port, data_dir, .. } | Command::RunHttp { port, data_dir } => {
            (*port, data_dir)
        }
    };
    let storage = Storage::open(data_dir)?;
    let address = SocketAddr::from(([0, 0, 0, 0], port));
    let handle = Handle::new();
    let shutdown_handle = handle.clone();

    tokio::spawn(async move {
        shutdown_signal().await;
        shutdown_handle.graceful_shutdown(Some(Duration::from_secs(10)));
    });

    match command {
        Command::RunHttps {
            certificate_path,
            key_path,
            data_dir,
            ..
        } => {
            rustls::crypto::ring::default_provider()
                .install_default()
                .map_err(|_| io::Error::other("could not install the rustls crypto provider"))?;
            let tls_config = RustlsConfig::from_pem_file(&certificate_path, &key_path).await?;
            info!(address = %address, data_dir = %data_dir.display(), "HTTPS server listening");
            axum_server::bind_rustls(address, tls_config)
                .handle(handle)
                .serve(router(storage).into_make_service())
                .await?;
        }
        Command::RunHttp { data_dir, .. } => {
            info!(address = %address, data_dir = %data_dir.display(), "HTTP server listening");
            axum_server::bind(address)
                .handle(handle)
                .serve(router(storage).into_make_service())
                .await?;
        }
        Command::Help => unreachable!("help exits before starting the server"),
    }

    Ok(())
}

fn parse_command() -> Result<Command, io::Error> {
    parse_command_from(pico_args::Arguments::from_env())
}

fn parse_command_from(mut arguments: pico_args::Arguments) -> Result<Command, io::Error> {
    if arguments.contains(["-h", "--help"]) {
        return Ok(Command::Help);
    }

    let port = arguments
        .opt_value_from_str("--port")
        .map_err(invalid_arguments)?
        .unwrap_or(3000);
    let data_dir = arguments
        .value_from_os_str("--data-dir", path_from_os_str)
        .map_err(invalid_arguments)?;
    let certificate_path = arguments
        .opt_value_from_os_str("--cert", path_from_os_str)
        .map_err(invalid_arguments)?;
    let key_path = arguments
        .opt_value_from_os_str("--key", path_from_os_str)
        .map_err(invalid_arguments)?;

    let unused = arguments.finish();
    if let Some(argument) = unused.first() {
        return Err(io::Error::new(
            io::ErrorKind::InvalidInput,
            format!(
                "unexpected argument: {}\n\n{HELP}",
                argument.to_string_lossy()
            ),
        ));
    }

    match (port, certificate_path, key_path) {
        (443, Some(certificate_path), Some(key_path)) => Ok(Command::RunHttps {
            port,
            certificate_path,
            key_path,
            data_dir,
        }),
        (_, Some(_), None) | (_, None, Some(_)) => Err(io::Error::new(
            io::ErrorKind::InvalidInput,
            format!("--cert and --key must be set together\n\n{HELP}"),
        )),
        _ => Ok(Command::RunHttp { port, data_dir }),
    }
}

fn path_from_os_str(value: &std::ffi::OsStr) -> Result<PathBuf, io::Error> {
    Ok(PathBuf::from(value))
}

fn invalid_arguments(error: pico_args::Error) -> io::Error {
    io::Error::new(io::ErrorKind::InvalidInput, format!("{error}\n\n{HELP}"))
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

#[cfg(test)]
mod tests {
    use super::{Command, parse_command_from};
    use std::{ffi::OsString, path::PathBuf};

    fn parse(arguments: &[&str]) -> Result<Command, std::io::Error> {
        parse_command_from(pico_args::Arguments::from_vec(
            arguments.iter().map(OsString::from).collect(),
        ))
    }

    #[test]
    fn defaults_to_http_on_port_3000() {
        assert_eq!(
            parse(&["--data-dir", "/var/lib/ble-scale"]).unwrap(),
            Command::RunHttp {
                port: 3000,
                data_dir: PathBuf::from("/var/lib/ble-scale"),
            },
        );
    }

    #[test]
    fn uses_https_on_port_443_with_a_certificate_and_key() {
        assert_eq!(
            parse(&[
                "--data-dir",
                "/var/lib/ble-scale",
                "--port",
                "443",
                "--cert",
                "cert.pem",
                "--key",
                "key.pem",
            ])
            .unwrap(),
            Command::RunHttps {
                port: 443,
                certificate_path: PathBuf::from("cert.pem"),
                key_path: PathBuf::from("key.pem"),
                data_dir: PathBuf::from("/var/lib/ble-scale"),
            },
        );
    }

    #[test]
    fn uses_http_when_tls_arguments_are_given_on_another_port() {
        assert_eq!(
            parse(&[
                "--data-dir",
                "data",
                "--port",
                "3001",
                "--cert",
                "cert.pem",
                "--key",
                "key.pem",
            ])
            .unwrap(),
            Command::RunHttp {
                port: 3001,
                data_dir: PathBuf::from("data"),
            },
        );
    }
}
