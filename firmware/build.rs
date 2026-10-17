use std::collections::HashMap;
use std::fs;
use std::path::PathBuf;

const REQUIRED_CONFIG: [&str; 4] = [
    "WIFI_SSID",
    "WIFI_PASSWORD",
    "HEALTHCHECK_URL",
    "REPORT_URL",
];

fn main() {
    println!("cargo:rerun-if-changed=.env");
    prepare_root_ca();
    let file_config = read_env_file();

    for key in REQUIRED_CONFIG {
        println!("cargo:rerun-if-env-changed={key}");
        let value = std::env::var(key)
            .ok()
            .or_else(|| file_config.get(key).cloned())
            .filter(|value| !value.is_empty())
            .unwrap_or_else(|| {
                panic!(
                    "missing required configuration {key}; set it in .env or in the build environment"
                )
            });

        println!("cargo:rustc-env={key}={value}");
    }

    embuild::espidf::sysenv::output();
}

fn prepare_root_ca() {
    const SOURCE: &str = "root-ca.cert.pem";
    const OUTPUT: &str = "report-root-ca.cert.pem";

    println!("cargo:rerun-if-changed={SOURCE}");

    let mut certificate = fs::read(SOURCE)
        .unwrap_or_else(|error| panic!("could not read reporting root CA {SOURCE}: {error}"));
    certificate.push(0);

    let output_dir = PathBuf::from(
        std::env::var_os("OUT_DIR").expect("Cargo did not provide an OUT_DIR to the build script"),
    );
    fs::write(output_dir.join(OUTPUT), certificate)
        .unwrap_or_else(|error| panic!("could not prepare reporting root CA: {error}"));
}

fn read_env_file() -> HashMap<String, String> {
    let Ok(contents) = fs::read_to_string(".env") else {
        return HashMap::new();
    };

    contents
        .lines()
        .filter_map(|line| {
            let line = line.trim();
            if line.is_empty() || line.starts_with('#') {
                return None;
            }

            let line = line.strip_prefix("export ").unwrap_or(line);
            let (key, value) = line.split_once('=')?;
            Some((key.trim().to_owned(), unquote(value.trim()).to_owned()))
        })
        .collect()
}

fn unquote(value: &str) -> &str {
    if value.len() >= 2
        && ((value.starts_with('"') && value.ends_with('"'))
            || (value.starts_with('\'') && value.ends_with('\'')))
    {
        &value[1..value.len() - 1]
    } else {
        value
    }
}
