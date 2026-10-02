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
    /// Verify a bundle, then replay it as OTLP/JSON to a collector or a file
    /// (TRC-28).
    Export {
        /// The bundle directory, `runs/<run_id>/`.
        dir: PathBuf,
        /// An OTLP/HTTP traces endpoint, e.g. `http://localhost:4318/v1/traces`.
        #[arg(
            long,
            conflicts_with = "otlp_json",
            required_unless_present = "otlp_json"
        )]
        otlp: Option<String>,
        /// Write the OTLP/JSON document to this file instead (never replaced).
        #[arg(long)]
        otlp_json: Option<PathBuf>,
    },
    /// Ingest an OTLP/JSON document (TRC-28): align foreign clocks (TRC-26) and
    /// write the four tables and the five views into a new directory. The result
    /// is not a run bundle: it has no manifest and no run_id.
    Import {
        /// The OTLP/JSON `ExportTraceServiceRequest` to read.
        #[arg(long)]
        otlp_json: PathBuf,
        /// The directory to create; it must not exist.
        #[arg(long)]
        out: PathBuf,
    },
}

/// Read a bundle's tables back as OTLP/JSON, after verifying it (TRC-23).
fn export_doc(dir: &std::path::Path) -> anyhow::Result<(String, String)> {
    let v = acn_trace::bundle::verify(dir)?;
    let inv = acn_trace::schema::inventory()?;
    let trace = acn_trace::parquet_io::read_trace(dir, &inv)?;
    let doc = acn_trace::otlp::to_json(&trace)?;
    Ok((v.run_id.to_hex(), serde_json::to_string(&doc)?))
}

/// POST a document to an OTLP/HTTP endpoint and require a 2xx answer.
fn post(endpoint: &str, body: String) -> anyhow::Result<u16> {
    let rt = tokio::runtime::Builder::new_current_thread()
        .enable_all()
        .build()?;
    rt.block_on(async {
        let resp = reqwest::Client::new()
            .post(endpoint)
            .header("content-type", "application/json")
            .body(body)
            .send()
            .await?;
        let status = resp.status();
        if !status.is_success() {
            anyhow::bail!("the collector answered {status}");
        }
        Ok(status.as_u16())
    })
}

fn bundle_export(
    dir: &std::path::Path,
    otlp: Option<&str>,
    otlp_json: Option<&std::path::Path>,
) -> Value {
    let result = (|| -> anyhow::Result<Value> {
        let (run_id, body) = export_doc(dir)?;
        let bytes = body.len();
        match (otlp, otlp_json) {
            (Some(endpoint), None) => {
                let status = post(endpoint, body)?;
                Ok(
                    json!({ "ok": true, "run_id": run_id, "endpoint": endpoint, "status": status, "bytes": bytes }),
                )
            }
            (None, Some(path)) => {
                use std::io::Write as _;
                let mut f = std::fs::OpenOptions::new()
                    .write(true)
                    .create_new(true)
                    .open(path)?;
                f.write_all(body.as_bytes())?;
                Ok(
                    json!({ "ok": true, "run_id": run_id, "file": path.display().to_string(), "bytes": bytes }),
                )
            }
            _ => anyhow::bail!("give exactly one of --otlp and --otlp-json"),
        }
    })();
    result.unwrap_or_else(|e| {
        tracing::error!(dir = %dir.display(), "{e:#}");
        json!({ "ok": false, "error": format!("{e:#}") })
    })
}

fn bundle_import(file: &std::path::Path, out: &std::path::Path) -> Value {
    let result = (|| -> anyhow::Result<Value> {
        let doc: Value = serde_json::from_slice(&std::fs::read(file)?)?;
        let trace = acn_trace::otlp::from_json(&doc)?;
        let trace = acn_trace::ingest::align_clocks(&trace)?;
        let inv = acn_trace::schema::inventory()?;
        let views = acn_trace::ingest::views(&inv, &acn_trace::schema::views()?, &trace)?;
        std::fs::create_dir(out)?;
        acn_trace::parquet_io::write_trace(out, &inv, &trace)?;
        for (view, batch) in &views {
            let path = out.join(&view.file);
            if let Some(parent) = path.parent() {
                std::fs::create_dir_all(parent)?;
            }
            acn_trace::parquet_io::write_batch(&path, batch)?;
        }
        Ok(json!({
            "ok": true,
            "out": out.display().to_string(),
            "spans": trace.spans.len(),
            "events": trace.events.len(),
            "links": trace.links.len(),
            "resources": trace.resources.len(),
        }))
    })();
    result.unwrap_or_else(|e| {
        tracing::error!(file = %file.display(), "{e:#}");
        json!({ "ok": false, "error": format!("{e:#}") })
    })
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
        Cmd::Bundle {
            cmd:
                BundleCmd::Export {
                    dir,
                    otlp,
                    otlp_json,
                },
        } => bundle_export(&dir, otlp.as_deref(), otlp_json.as_deref()),
        Cmd::Bundle {
            cmd: BundleCmd::Import { otlp_json, out },
        } => bundle_import(&otlp_json, &out),
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
