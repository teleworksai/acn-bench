//! `acn` — the acn-bench command line (CON-8: one JSON object on stdout,
//! logs on stderr, exit 0 iff `"ok": true`). Subcommands land with their specs:
//! `version` (T01), `bundle verify` (TRC-23, T02b), `harness run` (HAR-50, T04),
//! `hyp lint` (HYP-27, T05), `hyp verdict` (HYP-20, T05.2b), `loop run` (LOOP-10,
//! LOOP-14, T05b.1), `evidence verify` (LOOP-2, T05b.2).
#![forbid(unsafe_code)]

mod build_info;

use acn_cli::loop_exec;

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
    /// The agent harness (SPEC 040).
    Harness {
        #[command(subcommand)]
        cmd: HarnessCmd,
    },
    /// Hypothesis files and verdicts (SPEC 080).
    Hyp {
        #[command(subcommand)]
        cmd: HypCmd,
    },
    /// The layered feedback loop (SPEC 085).
    Loop {
        #[command(subcommand)]
        cmd: LoopCmd,
    },
    /// The evidence chain (SPEC 085 §4).
    Evidence {
        #[command(subcommand)]
        cmd: EvidenceCmd,
    },
}

#[derive(Subcommand)]
enum EvidenceCmd {
    /// Walk the chain from a loop report or a verdict down to its L1 bundles
    /// (LOOP-2): every bundle verifies with its views, the verdict recomputes to
    /// its bytes, every recorded layer is the derived one (LOOP-1), and the
    /// report regenerates byte for byte (LOOP-14).
    Verify {
        /// A loop_id, or the verdict_id of a loop's final verdict.
        id: String,
        /// The `runs` directory; resolved against the workspace root (CON-28).
        #[arg(long, default_value = "runs")]
        runs_dir: PathBuf,
    },
}

#[derive(Subcommand)]
enum LoopCmd {
    /// Run L1 for a hypothesis in sim on the mock: batches of one cell, the
    /// verdict after each, a final verdict and a loop report (LOOP-10, LOOP-11).
    /// With --from-report, regenerate a report and compare it byte for byte
    /// (LOOP-14).
    Run(Box<LoopRun>),
}

#[derive(clap::Args)]
struct LoopRun {
    /// The hypothesis file; its `[design].search` is the strategy.
    #[arg(
        long,
        required_unless_present = "from_report",
        conflicts_with = "from_report"
    )]
    hypothesis: Option<PathBuf>,
    /// The workload file, or `<value>=<file>` once per value when the file
    /// varies `workload`.
    #[arg(
        long = "workload",
        value_name = "FILE | VALUE=FILE",
        conflicts_with = "from_report"
    )]
    workload: Vec<String>,
    /// The mock profile (MLM-50), or `<value>=<profile>` once per value when the
    /// file varies `provider`.
    #[arg(
        long = "model",
        value_name = "PROFILE | VALUE=PROFILE",
        conflicts_with = "from_report"
    )]
    model: Vec<String>,
    /// How many bundles the loop may use, treatment and control alike.
    #[arg(
        long,
        required_unless_present = "from_report",
        conflicts_with = "from_report"
    )]
    budget: Option<u64>,
    /// The `runs` directory: bundles, `verdicts/` and `loop/` go under it. This
    /// path, the hypothesis, the workloads and --from-report resolve against
    /// the workspace root (CON-28), or the current directory outside one.
    #[arg(long, default_value = "runs", conflicts_with = "from_report")]
    runs_dir: PathBuf,
    /// A loop report, `runs/loop/<loop_id>/report.json`, to regenerate.
    #[arg(long)]
    from_report: Option<PathBuf>,
}

#[derive(Subcommand)]
enum HypCmd {
    /// Parse a hypothesis file, type-check its predicates, resolve its quantities
    /// and, for a frozen file, check that its falsifier can fire and fail to fire
    /// (HYP-27). Reads no bundle.
    Lint {
        /// The hypothesis file.
        file: PathBuf,
    },
    /// Verify bundles and judge them against a hypothesis: the only path from
    /// bundles to a verdict (HYP-20). Writes `runs/verdicts/<verdict_id>/verdict.json`.
    Verdict {
        /// The hypothesis file.
        #[arg(long)]
        hypothesis: PathBuf,
        /// The bundle directories, `runs/<run_id>/`, in any order.
        #[arg(required = true)]
        bundles: Vec<PathBuf>,
        /// The `runs` directory `verdicts/` goes under; it must be named `runs`
        /// (HYP-4).
        #[arg(long, default_value = "runs")]
        runs_dir: PathBuf,
    },
}

#[derive(Subcommand)]
enum HarnessCmd {
    /// Run one cell and one arm of a workload into one bundle (HAR-50).
    Run(Box<HarnessRun>),
}

#[derive(clap::Args)]
struct HarnessRun {
    /// The workload file (HAR-60).
    #[arg(long)]
    workload: PathBuf,
    /// mockllm, openai, vllm, sglang or anthropic (HAR-20).
    #[arg(long)]
    backend: String,
    /// The model requested; for mockllm, the profile name (CON-29).
    #[arg(long)]
    model: String,
    /// sim (mockllm only) or live (HAR-52).
    #[arg(long, default_value = "sim")]
    mode: String,
    /// treatment or control.
    #[arg(long, default_value = "treatment")]
    arm: String,
    #[arg(long, default_value_t = 1)]
    replicates: u32,
    /// `<name>=<value>`, once per varied parameter: a knob, or with a hypothesis
    /// any of its [varies] parameters (HAR-10).
    #[arg(long = "vary", value_name = "NAME=VALUE")]
    vary: Vec<String>,
    /// The hypothesis file; the seed is derived from it (HYP-9).
    #[arg(long, conflicts_with = "seed", required_unless_present = "seed")]
    hypothesis: Option<PathBuf>,
    /// The run seed, for a run with no hypothesis.
    #[arg(long)]
    seed: Option<u64>,
    /// `opt.endpoint`: the endpoint's base URL (HAR-25).
    #[arg(long, default_value = "")]
    endpoint: String,
    /// `opt.max_retries` (HAR-24).
    #[arg(long, default_value_t = 3)]
    max_retries: u64,
    /// `opt.retry_base_ms` (HAR-24).
    #[arg(long, default_value_t = 500)]
    retry_base_ms: u64,
    /// `opt.request_timeout_ms` (HAR-24).
    #[arg(long, default_value_t = 600_000)]
    request_timeout_ms: u64,
    /// `opt.stall_threshold_ms` (TRC-12).
    #[arg(long, default_value_t = 250.0)]
    stall_threshold_ms: f64,
    /// Where bundles go.
    #[arg(long, default_value = "runs")]
    runs_dir: PathBuf,
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

fn harness_run(a: &HarnessRun) -> Value {
    respond("harness run", || {
        let mut vary = std::collections::BTreeMap::new();
        for v in &a.vary {
            let (k, val) = v
                .split_once('=')
                .ok_or_else(|| anyhow::anyhow!("--vary `{v}` is not NAME=VALUE"))?;
            if vary.insert(k.to_owned(), val.to_owned()).is_some() {
                anyhow::bail!("--vary `{k}` is given twice");
            }
        }
        let hypothesis = match (&a.hypothesis, a.seed) {
            (Some(p), None) => acn_harness::run::HypothesisArg::File(p.clone()),
            (None, Some(seed)) => acn_harness::run::HypothesisArg::None { seed },
            _ => anyhow::bail!("give exactly one of --hypothesis and --seed"),
        };
        let cfg = acn_harness::run::RunConfig {
            workload: a.workload.clone(),
            backend: acn_harness::wire::Backend::parse(&a.backend)?,
            model: a.model.clone(),
            mode: acn_trace::identity::Mode::parse(&a.mode)?,
            arm: a.arm.clone(),
            replicates: a.replicates,
            vary,
            opts: acn_harness::agent::Opts {
                endpoint: a.endpoint.clone(),
                max_retries: a.max_retries,
                retry_base_ms: a.retry_base_ms,
                request_timeout_ms: a.request_timeout_ms,
                stall_threshold_ms: a.stall_threshold_ms,
            },
            hypothesis,
            runs_dir: a.runs_dir.clone(),
            start_dir: std::env::current_dir()?,
            engine_hash: build_info::engine_hash()?,
            build: build_info::build_info()?,
            profiles: None,
        };
        let w = acn_harness::run::run(&cfg)?;
        Ok(json!({
            "ok": true,
            "run_id": w.run_id.to_hex(),
            "bundle_digest": w.bundle_digest.to_hex(),
            "dir": w.dir.display().to_string(),
        }))
    })
}

/// HYP-27's parsed structure: parameters, measures with their units, control
/// and design, as loading resolved them.
fn hyp_structure(h: &acn_hyp::Hypothesis) -> Value {
    use acn_hyp::file::{Control, Domain, Tolerance};
    let params: serde_json::Map<String, Value> = h
        .params()
        .values()
        .map(|p| {
            let domain = match &p.domain {
                Domain::Bool => json!({ "kind": "bool" }),
                Domain::Enum(v) => json!({ "kind": "enum", "values": v }),
                Domain::Range { min, max, levels } => {
                    json!({ "kind": "range", "min": min, "max": max, "levels": levels })
                }
                Domain::IntRange { min, max, levels } => {
                    json!({ "kind": "int_range", "min": min, "max": max, "levels": levels })
                }
            };
            (
                p.name.clone(),
                json!({ "domain": domain, "pooled": p.pooled }),
            )
        })
        .collect();
    let unit = |q: &String| acn_hyp::quantities::get(q).map(|x| x.unit);
    let measures = |qs: &[String]| -> Vec<Value> {
        qs.iter()
            .map(|q| json!({ "name": q, "unit": unit(q) }))
            .collect()
    };
    let control = match &h.control() {
        Control::Config(c) => json!({
            "config": c.iter().map(|(k, v)| (k.clone(), json!(v.to_string()))).collect::<serde_json::Map<_, _>>()
        }),
        Control::Workload { mode, inherits } => json!({ "workload": mode, "inherits": inherits }),
        Control::Missing => Value::Null,
    };
    let d = &h.design();
    let tolerance: serde_json::Map<String, Value> = d
        .sim_live_tolerance
        .iter()
        .map(|(q, t)| {
            let v = match t {
                Tolerance::Relative(x) => json!({ "relative": x }),
                Tolerance::Absolute(x) => json!({ "abs": x }),
            };
            (q.clone(), v)
        })
        .collect();
    json!({
        "params": params,
        "measures": { "primary": measures(h.primary()), "secondary": measures(h.secondary()) },
        "control": control,
        "design": {
            "search": d.search, "replicates": d.replicates, "twin_required": d.twin_required,
            "seed": d.seed.map(|s| s.to_string()), "backends": d.backends,
            "min_providers_for_verdict": d.min_providers_for_verdict,
            "sim_live_tolerance": tolerance, "pinned": d.pins.is_some(),
        },
        "expected": h.expected_outcome(),
    })
}

fn hyp_lint(file: &std::path::Path) -> Value {
    let r = acn_hyp::lint::lint(file);
    let structure = acn_hyp::load(file).ok().map(|h| hyp_structure(&h));
    for w in &r.warnings {
        tracing::warn!(file = %file.display(), "{w}");
    }
    for e in &r.errors {
        tracing::error!(file = %file.display(), "{e}");
    }
    json!({
        "ok": r.ok(),
        "file": file.display().to_string(),
        "id": r.id,
        "status": r.status.map(acn_hyp::Status::as_str),
        "hash": r.hash,
        "predicate": r.predicate,
        "guard": r.guard,
        "errors": r.errors,
        "warnings": r.warnings,
        "witness": { "fires": r.fires, "holds": r.holds },
        "structure": structure,
    })
}

/// `acn hyp verdict` (HYP-20): `ok` reports that the evaluation completed,
/// whatever the verdict; a refusal is `ok: false` with its reason.
fn hyp_verdict(file: &std::path::Path, dirs: &[PathBuf], runs_dir: &std::path::Path) -> Value {
    respond("hyp verdict", || {
        let h = acn_hyp::load(file)?;
        let bundles = dirs
            .iter()
            .map(|d| acn_hyp::read::read(d))
            .collect::<Result<Vec<_>, _>>()?;
        // HYP-4: judged, then the file re-read before the write.
        let (v, path) =
            acn_hyp::verdict::judge_and_write(&h, bundles, build_info::engine_hash()?, runs_dir)?;
        let object: Value = serde_json::from_str(&v.text())?;
        Ok(json!({
            "ok": true,
            "verdict_id": v.verdict_id.to_hex(),
            "verdict_path": path.display().to_string(),
            "run_ids": v.bundles.iter().map(|(r, _, _)| r.to_hex()).collect::<Vec<_>>(),
            "verdict": object,
        }))
    })
}

fn loop_failure(e: &acn_hyp::loop_run::LoopError) -> Value {
    tracing::error!("{e}");
    json!({ "ok": false, "code": e.code.as_str(), "error": e.to_string() })
}

/// The binary's identity (CON-31), the harness executor (LOOP-15), and the
/// directory paths resolve against: the workspace root, or the current
/// directory outside one (LOOP-10, CON-28).
fn loop_setup() -> anyhow::Result<(
    acn_hyp::loop_run::Binary,
    loop_exec::HarnessExecutor,
    std::path::PathBuf,
)> {
    let engine_hash = build_info::engine_hash()?;
    let build = build_info::build_info()?;
    let bin = acn_hyp::loop_run::Binary {
        engine_hash,
        build_hash: acn_trace::identity::Digest::from_hex(&build.build_hash)?,
    };
    let exec = loop_exec::HarnessExecutor { engine_hash, build };
    let cwd = std::env::current_dir()?;
    let root = acn_trace::env::find_root(&cwd)?.unwrap_or(cwd);
    Ok((bin, exec, root))
}

fn loop_run(a: &LoopRun) -> Value {
    use acn_hyp::loop_run::{self, Args};
    let (bin, mut exec, root) = match loop_setup() {
        Ok(x) => x,
        Err(e) => return json!({ "ok": false, "code": "internal", "error": format!("{e:#}") }),
    };
    if let Some(report) = &a.from_report {
        return match loop_run::regenerate(&root.join(report), bin, &mut exec) {
            Ok(r) => {
                if !r.identical() {
                    tracing::error!(differ = ?r.differ, "the regeneration differs (LOOP-14)");
                }
                json!({
                    "ok": r.identical(),
                    "identical": r.identical(),
                    "loop_id": r.loop_id.to_hex(),
                    "dir": r.dir.display().to_string(),
                    "differ": r.differ,
                })
            }
            Err(e) => loop_failure(&e),
        };
    }
    let (Some(file), Some(budget)) = (&a.hypothesis, a.budget) else {
        return json!({
            "ok": false,
            "code": "bad_args",
            "error": "give --hypothesis and --budget, or --from-report",
        });
    };
    let h = match acn_hyp::load_in(&root.join(file), &root) {
        Ok(h) => h,
        Err(e) => {
            return json!({ "ok": false, "code": "hypothesis_refused", "error": e.to_string() });
        }
    };
    let args = Args {
        workloads: a.workload.clone(),
        models: a.model.clone(),
        budget,
    };
    match loop_run::run(&h, &args, &root.join(&a.runs_dir), bin, &mut exec) {
        Ok(c) => json!({
            "ok": true,
            "loop_id": c.loop_id.to_hex(),
            "report": c.report.display().to_string(),
            "verdict_id": c.verdict_id.to_hex(),
            "verdict": c.verdict.as_str(),
            "stop": c.stop.as_str(),
            "run_ids": c.run_ids.iter().map(acn_trace::identity::Digest::to_hex).collect::<Vec<_>>(),
        }),
        Err(e) => loop_failure(&e),
    }
}

/// `acn evidence verify` (LOOP-2): `ok` iff every link of every chain holds.
fn evidence_verify(id: &str, runs_dir: &std::path::Path) -> Value {
    let (bin, mut exec, root) = match loop_setup() {
        Ok(x) => x,
        Err(e) => return json!({ "ok": false, "code": "internal", "error": format!("{e:#}") }),
    };
    match acn_hyp::evidence::verify(&root.join(runs_dir), id, bin, &mut exec) {
        Ok(c) => {
            for f in &c.findings {
                tracing::error!(code = f.code.as_str(), "{}", f.message);
            }
            json!({
                "ok": c.ok(),
                "target": c.target,
                "loops": c.loops,
                "bundles": c.bundles,
                "verdicts": c.verdicts,
                "regenerated": c.regenerated.iter().map(|p| p.display().to_string()).collect::<Vec<_>>(),
                "findings": c.findings.iter().map(|f| json!({ "code": f.code.as_str(), "message": f.message })).collect::<Vec<_>>(),
            })
        }
        Err(e) => loop_failure(&e),
    }
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
        Cmd::Harness {
            cmd: HarnessCmd::Run(a),
        } => harness_run(&a),
        Cmd::Hyp {
            cmd: HypCmd::Lint { file },
        } => hyp_lint(&file),
        Cmd::Hyp {
            cmd:
                HypCmd::Verdict {
                    hypothesis,
                    bundles,
                    runs_dir,
                },
        } => hyp_verdict(&hypothesis, &bundles, &runs_dir),
        Cmd::Loop {
            cmd: LoopCmd::Run(a),
        } => loop_run(&a),
        Cmd::Evidence {
            cmd: EvidenceCmd::Verify { id, runs_dir },
        } => evidence_verify(&id, &runs_dir),
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

#[cfg(test)]
mod tests {
    #![allow(clippy::unwrap_used, clippy::expect_used, clippy::panic)] // CON-19: tests are exempt
    use super::Cli;
    use clap::CommandFactory as _;

    /// Every argument of a subcommand, hidden or not: its id and its long and
    /// short names. (clap's `env` feature is off, so none reads the environment.)
    fn args(cmd: &clap::Command) -> Vec<String> {
        let mut v: Vec<String> = cmd
            .get_arguments()
            .filter(|a| a.get_id() != "help")
            .map(|a| {
                format!(
                    "{}|{}|{}",
                    a.get_id(),
                    a.get_long().unwrap_or(""),
                    a.get_short().map(String::from).unwrap_or_default(),
                )
            })
            .collect();
        v.sort();
        v
    }

    /// Cites: HYP-25
    #[test]
    fn acn_hyp_offers_no_option_to_relax_a_hypothesis() {
        let cli = Cli::command();
        let hyp = cli.find_subcommand("hyp").unwrap();
        let subs: Vec<&str> = hyp.get_subcommands().map(clap::Command::get_name).collect();
        assert_eq!(subs, ["lint", "verdict"], "hidden subcommands included");
        assert_eq!(args(hyp), Vec::<String>::new());
        assert_eq!(args(hyp.find_subcommand("lint").unwrap()), ["file||"]);
        assert_eq!(
            args(hyp.find_subcommand("verdict").unwrap()),
            ["bundles||", "hypothesis|hypothesis|", "runs_dir|runs-dir|"],
            "the file, the bundles and where the verdict goes: nothing that changes it"
        );
    }
}
