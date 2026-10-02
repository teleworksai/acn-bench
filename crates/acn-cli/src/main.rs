//! `acn` — the acn-bench command line (CON-8: one JSON object on stdout,
//! logs on stderr, exit 0 iff `"ok": true`). Subcommands land with their specs:
//! `version` (T01), `bundle verify` (TRC-23, T02b).
#![forbid(unsafe_code)]

mod build_info;

use std::path::PathBuf;

use clap::{Parser, Subcommand};
use serde_json::{Value, json};
use std::io::IsTerminal as _;
use tracing_subscriber::EnvFilter;

#[derive(Parser)]
#[command(
    name = "acn",
    version,
    about = "ACN experimental substrate",
    disable_help_subcommand = true
)]
struct Cli {
    #[command(subcommand)]
    cmd: Cmd,
}

#[derive(Subcommand)]
enum Cmd {
    /// Print the version of the `acn` binary, its build_hash (CON-31) and the
    /// engine_hash of the frozen code it was built from (CON-28).
    Version,
    /// Run bundles (SPEC 010 §5).
    Bundle {
        #[command(subcommand)]
        cmd: BundleCmd,
    },
}

#[derive(Subcommand)]
enum BundleCmd {
    /// Recompute a bundle's run_id and the hash of every listed file, and print its
    /// bundle_digest (TRC-23).
    Verify {
        /// The bundle directory, `runs/<run_id>/`.
        dir: PathBuf,
        /// Also recompute the five views from the tables and compare them (TRC-35).
        #[arg(long)]
        views: bool,
    },
}

fn version() -> Value {
    match (build_info::build_info(), build_info::engine_hash()) {
        (Ok(b), Ok(e)) => json!({
            "ok": true,
            "version": env!("CARGO_PKG_VERSION"),
            "build_hash": b.build_hash,
            "build": b,
            "engine_hash": e.to_hex(),
        }),
        (Err(e), _) | (_, Err(e)) => json!({ "ok": false, "error": e.to_string() }),
    }
}

fn bundle_verify(dir: &std::path::Path, views: bool) -> Value {
    let result = if views {
        acn_trace::bundle::verify_views(dir)
    } else {
        acn_trace::bundle::verify(dir)
    };
    match result {
        Ok(v) => json!({
            "ok": true,
            "run_id": v.run_id.to_hex(),
            "bundle_digest": v.bundle_digest.to_hex(),
            "files": v.files,
            "views_recomputed": views,
        }),
        Err(e) => {
            tracing::error!(dir = %dir.display(), "{e}");
            json!({ "ok": false, "error": e.to_string() })
        }
    }
}

/// Same policy as `xtask::logging::init` (kept in step by hand until a shared
/// home exists): stderr only, JSON when `ACN_LOG=json`, never panics.
fn init_logging() {
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
        eprintln!("acn: logging already initialised: {e}");
    }
}

fn run() -> Value {
    let cli = match Cli::try_parse() {
        Ok(cli) => cli,
        Err(e) => {
            use clap::error::ErrorKind::{DisplayHelp, DisplayVersion};
            if matches!(e.kind(), DisplayHelp | DisplayVersion) {
                eprint!("{e}");
                return json!({ "ok": true });
            }
            return json!({ "ok": false, "error": e.to_string() });
        }
    };
    match cli.cmd {
        Cmd::Version => version(),
        Cmd::Bundle {
            cmd: BundleCmd::Verify { dir, views },
        } => bundle_verify(&dir, views),
    }
}

fn main() {
    init_logging();
    let out = run();
    let ok = out.get("ok").and_then(Value::as_bool) == Some(true);
    // CON-8: the JSON object is the result. If it cannot be written (a closed pipe),
    // the run has not succeeded, and `println!` would panic instead of saying so.
    let written = {
        use std::io::Write as _;
        let mut stdout = std::io::stdout().lock();
        writeln!(stdout, "{out}")
            .and_then(|()| stdout.flush())
            .is_ok()
    };
    std::process::exit(if ok && written { 0 } else { 1 });
}
