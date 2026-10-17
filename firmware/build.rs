use std::collections::HashMap;
use std::fs;

const REQUIRED_CONFIG: [&str; 3] = ["WIFI_SSID", "WIFI_PASSWORD", "HEALTHCHECK_URL"];

fn main() {
    println!("cargo:rerun-if-changed=.env");
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
