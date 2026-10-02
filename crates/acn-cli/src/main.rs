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
        /// Spans per request to a collector, so no request outgrows a collector's
        /// body limit; the split is deterministic.
        #[arg(long, default_value_t = 2000)]
        max_spans: usize,
        /// Give up on a collector after this many seconds per request.
        #[arg(long, default_value_t = 30)]
        timeout_secs: u64,
    },
    /// Ingest an OTLP/JSON document (TRC-28): merge it with a bundle's own trace
    /// when given one, align foreign clocks (TRC-26), and write the four tables
    /// and the five views into a new directory. The result is not a run bundle:
    /// it has no manifest and no run_id.
    Import {
        /// The OTLP/JSON `ExportTraceServiceRequest` to read, e.g. an inference
        /// node's own export.
        #[arg(long)]
        otlp_json: PathBuf,
        /// A bundle whose trace the document is merged into: the run whose calls
        /// the node's spans answer.
        #[arg(long)]
        with: Option<PathBuf>,
        /// The directory to create; it must not exist.
        #[arg(long)]
        out: PathBuf,
    },
}

/// Run a fallible command body and turn an error into the CON-8 failure object.
fn respond(context: &str, body: impl FnOnce() -> anyhow::Result<Value>) -> Value {
    body().unwrap_or_else(|e| {
        tracing::error!(context, "{e:#}");
        json!({ "ok": false, "error": format!("{e:#}") })
    })
}

/// An endpoint as it may be shown: without credentials.
fn shown(endpoint: &str) -> String {
    match reqwest::Url::parse(endpoint) {
        Ok(mut url) => {
            let _ = url.set_username("");
            let _ = url.set_password(None);
            url.to_string()
        }
        Err(_) => "<unparseable endpoint>".to_owned(),
    }
}

/// The bundle's tables as OTLP/JSON documents, after verifying it (TRC-23).
fn export_docs(dir: &std::path::Path, max_spans: usize) -> anyhow::Result<(String, Vec<String>)> {
    let v = acn_trace::bundle::verify(dir)?;
    let inv = acn_trace::schema::inventory()?;
    let trace = acn_trace::parquet_io::read_trace(dir, &inv)?;
    let docs = acn_trace::otlp::to_json_chunks(&trace, max_spans)?
        .iter()
        .map(serde_json::to_string)
        .collect::<Result<Vec<_>, _>>()?;
    Ok((v.run_id.to_hex(), docs))
}

/// POST each document to an OTLP/HTTP endpoint in order. Redirects are refused (a
/// redirected POST becomes a GET that delivers nothing), every request has a
/// deadline, and anything but a 2xx is an error.
fn post_all(endpoint: &str, docs: Vec<String>, timeout: std::time::Duration) -> anyhow::Result<()> {
    let rt = tokio::runtime::Builder::new_current_thread()
        .enable_all()
        .build()?;
    rt.block_on(async {
        let client = reqwest::Client::builder()
            .connect_timeout(timeout)
            .timeout(timeout)
            .redirect(reqwest::redirect::Policy::none())
            .build()?;
        for (i, body) in docs.into_iter().enumerate() {
            let resp = client
                .post(endpoint)
                .header("content-type", "application/json")
                .body(body)
                .send()
                .await?;
            let status = resp.status();
            if !status.is_success() {
                anyhow::bail!("the collector answered {status} to request {}", i + 1);
            }
        }
        Ok(())
    })
}

/// Write a file that must not exist, never leaving a partial one: write a
/// sibling temporary file, then link it into place (linking fails if the target
/// exists), then remove the temporary name.
fn write_new_atomically(path: &std::path::Path, bytes: &[u8]) -> anyhow::Result<()> {
    use std::io::Write as _;
    let name = path
        .file_name()
        .and_then(|n| n.to_str())
        .ok_or_else(|| anyhow::anyhow!("{} has no file name", path.display()))?;
    let tmp = path.with_file_name(format!(".{name}.partial"));
    let result = (|| -> anyhow::Result<()> {
        let mut f = std::fs::OpenOptions::new()
            .write(true)
            .create_new(true)
            .open(&tmp)?;
        f.write_all(bytes)?;
        f.sync_all()?;
        std::fs::hard_link(&tmp, path).map_err(|e| anyhow::anyhow!("{}: {e}", path.display()))?;
        Ok(())
    })();
    let _ = std::fs::remove_file(&tmp);
    result
}

fn bundle_export(
    dir: &std::path::Path,
    otlp: Option<&str>,
    otlp_json: Option<&std::path::Path>,
    max_spans: usize,
    timeout_secs: u64,
) -> Value {
    respond("bundle export", || {
        match (otlp, otlp_json) {
            (Some(endpoint), None) => {
                let (run_id, docs) = export_docs(dir, max_spans)?;
                let (requests, bytes) = (docs.len(), docs.iter().map(String::len).sum::<usize>());
                post_all(endpoint, docs, std::time::Duration::from_secs(timeout_secs))?;
                Ok(json!({
                    "ok": true, "run_id": run_id, "endpoint": shown(endpoint),
                    "requests": requests, "bytes": bytes,
                }))
            }
            (None, Some(path)) => {
                let (run_id, mut docs) = export_docs(dir, 0)?;
                let body = docs.pop().ok_or_else(|| anyhow::anyhow!("no document"))?;
                write_new_atomically(path, body.as_bytes())?;
                Ok(json!({
                    "ok": true, "run_id": run_id, "file": path.display().to_string(),
                    "bytes": body.len(),
                }))
            }
            // clap enforces exactly one of the two.
            _ => anyhow::bail!("give exactly one of --otlp and --otlp-json"),
        }
    })
}

fn bundle_import(
    file: &std::path::Path,
    with: Option<&std::path::Path>,
    out: &std::path::Path,
) -> Value {
    respond("bundle import", || {
        let inv = acn_trace::schema::inventory()?;
        let doc: Value = serde_json::from_slice(&std::fs::read(file)?)?;
        let mut trace = acn_trace::otlp::from_json(&doc)?;
        if let Some(bundle) = with {
            acn_trace::bundle::verify(bundle)?;
            trace = acn_trace::parquet_io::read_trace(bundle, &inv)?
                .merge(trace)
                .map_err(|e| anyhow::anyhow!(e))?;
        }
        let trace = acn_trace::ingest::align_clocks(&trace)?;
        // Encoded and checked in full before the directory exists; written to a
        // temporary sibling and renamed into place, so a failure leaves nothing.
        let files = acn_trace::bundle::encode_tables_and_views(&inv, &trace)?;
        if out.exists() {
            anyhow::bail!(
                "{} exists; an import never writes into an existing directory",
                out.display()
            );
        }
        let name = out
            .file_name()
            .and_then(|n| n.to_str())
            .ok_or_else(|| anyhow::anyhow!("{} has no directory name", out.display()))?;
        let tmp = out.with_file_name(format!(".{name}.partial"));
        std::fs::create_dir(&tmp)?;
        let written = acn_trace::bundle::write_files(&tmp, &files)
            .map_err(anyhow::Error::from)
            .and_then(|()| std::fs::rename(&tmp, out).map_err(anyhow::Error::from));
        if written.is_err() {
            let _ = std::fs::remove_dir_all(&tmp);
        }
        written?;
        let aligned = trace
            .spans
            .iter()
            .filter(|s| s.attrs.contains_key(acn_trace::ingest::CLOCK_OFFSET))
            .count();
        Ok(json!({
            "ok": true,
            "out": out.display().to_string(),
            "spans": trace.spans.len(),
            "aligned_spans": aligned,
            "events": trace.events.len(),
            "links": trace.links.len(),
            "resources": trace.resources.len(),
        }))
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
                    max_spans,
                    timeout_secs,
                },
        } => bundle_export(
            &dir,
            otlp.as_deref(),
            otlp_json.as_deref(),
            max_spans,
            timeout_secs,
        ),
        Cmd::Bundle {
            cmd:
                BundleCmd::Import {
                    otlp_json,
                    with,
                    out,
                },
        } => bundle_import(&otlp_json, with.as_deref(), &out),
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
