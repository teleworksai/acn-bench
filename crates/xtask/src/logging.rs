//! stderr logging via `tracing` (CON-8): plain text by default, JSON when
//! `ACN_LOG=json`, otherwise `ACN_LOG` is an env-filter directive.

use std::io::IsTerminal as _;

use tracing_subscriber::EnvFilter;

/// Install the global subscriber. Safe to call once per process.
pub fn init() {
    let spec = std::env::var("ACN_LOG").unwrap_or_else(|_| "info".to_owned());
    let builder = tracing_subscriber::fmt()
        .with_writer(std::io::stderr)
        .with_ansi(std::io::stderr().is_terminal())
        .without_time();
    let result = if spec == "json" {
        builder
            .json()
            .with_env_filter(EnvFilter::new("info"))
            .try_init()
    } else {
        builder.with_env_filter(EnvFilter::new(spec)).try_init()
    };
    if let Err(e) = result {
        eprintln!("xtask: logging already initialised: {e}");
    }
}
