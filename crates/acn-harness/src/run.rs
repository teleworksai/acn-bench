//! One run (HAR-50..52): one cell and one arm of a workload, every replicate in
//! its seeded order (HAR-43), into one bundle.

use std::collections::BTreeMap;
use std::path::{Path, PathBuf};
use std::sync::Arc;

use acn_mockllm::Mock;
use acn_mockllm::profile::Profiles;
use acn_trace::bundle::{Bundle, HypothesisRef, RunSpec, Written};
use acn_trace::env::RunHypothesis;
use acn_trace::identity::{BuildInfo, Digest, HypStatus, Mode, Preimage, RunParams, Value};
use acn_trace::otel::{Collector, producer_resource};
use opentelemetry::KeyValue;
use opentelemetry::trace::TracerProvider as _;
use opentelemetry_sdk::trace::SdkTracerProvider;

use crate::HarnessError;
use crate::agent::{Counting, Opts, Replicate, Setup, Streams, permutation};
use crate::env::{Env, LiveEnv, SimEnv};
use crate::knobs::{Domain, Knobs};
use crate::wire::Backend;
use crate::workload::Workload;

/// Where a run's seed and hypothesis come from (HAR-50, HYP-9).
#[derive(Debug, Clone)]
pub enum HypothesisArg {
    /// No hypothesis: the seed is given.
    None { seed: u64 },
    /// A hypothesis file: the seed is derived from it.
    File(PathBuf),
}

/// Everything `acn harness run` is told.
#[derive(Debug, Clone)]
pub struct RunConfig {
    pub workload: PathBuf,
    pub backend: Backend,
    pub model: String,
    pub mode: Mode,
    /// `treatment` or `control`.
    pub arm: String,
    pub replicates: u32,
    /// `vary.<name>` values as given, keyed without the prefix; typed by the
    /// hypothesis's `[varies]` kinds, or by the knobs' own types without one.
    pub vary: BTreeMap<String, String>,
    pub opts: Opts,
    pub hypothesis: HypothesisArg,
    pub runs_dir: PathBuf,
    /// Where the workspace root is looked for (CON-28).
    pub start_dir: PathBuf,
    /// The `engine_hash` the binary embedded (CON-28, CON-31).
    pub engine_hash: Digest,
    pub build: BuildInfo,
    /// The mock's profiles in `sim`; `None` means the embedded ones (MLM-50).
    pub profiles: Option<Profiles>,
}

/// The isolation marker of HAR-42.
pub fn isolation_marker(run_id: &Digest, arm: &str, i: u32) -> Result<String, HarnessError> {
    let d = Preimage::new("acn-bench/harness_isolation/v1")?
        .digest(run_id)
        .str(arm)?
        .u32(i)
        .finish();
    Ok(d.to_hex()[..16].to_owned())
}

/// What a hypothesis file contributes to a run.
struct Hyp {
    run: RunHypothesis,
    reference: HypothesisRef,
    status: HypStatus,
    seed: u64,
    /// `[varies]` parameter name → domain, when there is a hypothesis file.
    varies: Option<BTreeMap<String, Domain>>,
}

/// HYP-9: a frozen file's seed, and a candidate's without `[design].seed`.
fn hypothesis_seed(hash: &Digest) -> Result<u64, HarnessError> {
    Ok(acn_trace::identity::hypothesis_seed(hash)?)
}

fn hypothesis(arg: &HypothesisArg, start: &Path) -> Result<Hyp, HarnessError> {
    let path = match arg {
        HypothesisArg::None { seed } => {
            return Ok(Hyp {
                run: RunHypothesis::None,
                reference: HypothesisRef::none(),
                status: HypStatus::Candidate,
                seed: *seed,
                varies: None,
            });
        }
        HypothesisArg::File(p) => p,
    };
    let bad = |m: String| HarnessError::Config(format!("{}: {m}", path.display()));
    let bytes = std::fs::read(path).map_err(|e| bad(e.to_string()))?;
    let hash = Digest::of(&bytes);
    let doc: toml::Table = std::str::from_utf8(&bytes)
        .map_err(|e| bad(e.to_string()))
        .and_then(|t| toml::from_str(t).map_err(|e| bad(e.to_string())))?;
    let id = doc
        .get("poc")
        .and_then(|p| p.get("id"))
        .and_then(toml::Value::as_str)
        .ok_or_else(|| bad("`[poc].id` is missing".into()))?
        .to_owned();
    // TODO(T05): replace this reading with acn-hyp's typed loader (SPEC 080).
    let varies = match doc.get("varies").and_then(toml::Value::as_table) {
        Some(t) => Some(
            t.iter()
                .map(|(k, v)| Domain::parse(k, v).map(|d| (k.clone(), d)))
                .collect::<Result<BTreeMap<_, _>, _>>()?,
        ),
        None => None,
    };
    // HYP-3: frozen when it lies under `<root>/hypotheses/`; the preflight then
    // checks that `env-hash.json` records it with this hash.
    let abs = std::fs::canonicalize(path).map_err(|e| bad(e.to_string()))?;
    let frozen = acn_trace::env::find_root(start)?
        .is_some_and(|root| abs.starts_with(root.join("hypotheses")));
    let design_seed = doc
        .get("design")
        .and_then(|d| d.get("seed"))
        .and_then(toml::Value::as_integer);
    let seed = match (frozen, design_seed) {
        (true, Some(_)) => {
            return Err(bad(
                "a frozen file carries no `[design].seed` (HYP-9)".into()
            ));
        }
        (false, Some(s)) => {
            u64::try_from(s).map_err(|_| bad("`[design].seed` is negative".into()))?
        }
        (_, None) => hypothesis_seed(&hash)?,
    };
    let (run, status) = if frozen {
        (
            RunHypothesis::Frozen {
                path: abs.clone(),
                hash,
            },
            HypStatus::Frozen,
        )
    } else {
        (RunHypothesis::Candidate { hash }, HypStatus::Candidate)
    };
    Ok(Hyp {
        run,
        reference: HypothesisRef { id, hash },
        status,
        seed,
        varies,
    })
}

/// The base URL a live run calls, and its host for the manifest (CON-26).
fn endpoint(backend: Backend, opts: &Opts) -> Result<(String, String), HarnessError> {
    let url = if opts.endpoint.is_empty() {
        match backend {
            Backend::Openai => "https://api.openai.com".to_owned(),
            Backend::Anthropic => "https://api.anthropic.com".to_owned(),
            _ => {
                return Err(HarnessError::Config(format!(
                    "a live `{}` run needs --endpoint",
                    backend.as_str()
                )));
            }
        }
    } else {
        opts.endpoint.clone()
    };
    let parsed =
        reqwest::Url::parse(&url).map_err(|e| HarnessError::Config(format!("--endpoint: {e}")))?;
    if !parsed.username().is_empty() || parsed.password().is_some() {
        return Err(HarnessError::Config(
            "--endpoint carries credentials; they come from the environment (HAR-22)".into(),
        ));
    }
    // A base URL only: a query could carry a key into the manifest (HAR-22).
    if parsed.query().is_some() || parsed.fragment().is_some() {
        return Err(HarnessError::Config(
            "--endpoint is a base URL, with no query or fragment (HAR-22)".into(),
        ));
    }
    let host = parsed
        .host_str()
        .ok_or_else(|| HarnessError::Config("--endpoint has no host".into()))?
        .to_owned();
    Ok((url, host))
}

/// The headers of every live request: the tenant on the mock (HAR-42), the
/// credentials on a provider (HAR-22).
fn headers(backend: Backend, marker: &str) -> Result<Vec<(String, String)>, HarnessError> {
    match backend {
        Backend::Mockllm => Ok(vec![("authorization".into(), marker.to_owned())]),
        #[cfg(feature = "real-api")]
        b => crate::credentials::headers(b),
        #[cfg(not(feature = "real-api"))]
        b => Err(HarnessError::Config(format!(
            "backend `{}` needs a build with the `real-api` feature (HAR-20)",
            b.as_str()
        ))),
    }
}

/// Run one cell and arm into one bundle (HAR-50), on a runtime of its own. From
/// inside a tokio runtime, await [`run_async`] instead.
pub fn run(cfg: &RunConfig) -> Result<Written, HarnessError> {
    tokio::runtime::Builder::new_current_thread()
        .enable_all()
        .build()
        .map_err(|e| HarnessError::Internal(format!("runtime: {e}")))?
        .block_on(run_async(cfg))
}

/// [`run`] on the caller's runtime. The future is not `Send`: the agent's
/// lineages share their state through `RefCell`s, so it is awaited in place (or
/// on a `LocalSet`), never spawned onto another thread.
pub async fn run_async(cfg: &RunConfig) -> Result<Written, HarnessError> {
    if !matches!(cfg.arm.as_str(), "treatment" | "control") {
        return Err(HarnessError::Config(format!(
            "--arm is treatment or control, not `{}`",
            cfg.arm
        )));
    }
    if cfg.replicates == 0 {
        return Err(HarnessError::Config("--replicates must be positive".into()));
    }
    match cfg.mode {
        Mode::Sim if cfg.backend != Backend::Mockllm => {
            return Err(HarnessError::Config(
                "sim has no sockets: only the mockllm backend runs in sim (HAR-52)".into(),
            ));
        }
        Mode::Netem => {
            return Err(HarnessError::Config(
                "netem is not defined until SPEC 020 (HAR-52)".into(),
            ));
        }
        _ => {}
    }
    let workload = Workload::load(&cfg.workload)?;
    let hyp = hypothesis(&cfg.hypothesis, &cfg.start_dir)?;
    let vary = typed_vary(&cfg.vary, hyp.varies.as_ref())?;
    let knobs = Knobs::from_vary(&vary)?;
    let profiles = match &cfg.profiles {
        Some(p) => p.clone(),
        None => {
            acn_mockllm::profile::embedded().map_err(|e| HarnessError::Config(e.to_string()))?
        }
    };
    let counting = if cfg.backend == Backend::Mockllm {
        let p = profiles.get(&cfg.model).ok_or_else(|| {
            HarnessError::Config(format!("`{}` is not a mock profile (MLM-50)", cfg.model))
        })?;
        Counting::Tokens(Box::new(p.clone()))
    } else {
        Counting::BytesScaled
    };
    let pf = acn_trace::env::preflight(&cfg.start_dir, cfg.engine_hash, hyp.run.clone())?;

    let live = cfg.mode == Mode::Live;
    let clock = Arc::new(acn_emu::clock::WallClock::start());
    let (url, host) = if live {
        let (u, h) = endpoint(cfg.backend, &cfg.opts)?;
        (u, Some(h))
    } else {
        (String::new(), None)
    };
    if live {
        // HAR-23: the endpoint says what it is before the run gets an identity.
        let probe = LiveEnv::new(Arc::clone(&clock), &url, headers(cfg.backend, "-")?)?;
        let is_mock = probe.probe_is_mock().await?;
        if is_mock != (cfg.backend == Backend::Mockllm) {
            return Err(HarnessError::BackendMismatch(format!(
                "configured `{}`, but the endpoint {} the mock's marker (CON-26, HAR-23)",
                cfg.backend.as_str(),
                if is_mock { "shows" } else { "does not show" }
            )));
        }
    }

    let mut order_rng = acn_trace::identity::substream_rng(hyp.seed, "run.order")?;
    let order: Vec<u32> = permutation(&mut order_rng, cfg.replicates as usize)
        .into_iter()
        .map(|i| u32::try_from(i).unwrap_or(0))
        .collect();
    let mut opts = BTreeMap::new();
    opts.insert("opt.endpoint".into(), Value::Str(cfg.opts.endpoint.clone()));
    opts.insert(
        "opt.max_retries".into(),
        Value::Int(int(cfg.opts.max_retries)),
    );
    opts.insert(
        "opt.retry_base_ms".into(),
        Value::Float(float(cfg.opts.retry_base_ms)),
    );
    opts.insert(
        "opt.request_timeout_ms".into(),
        Value::Float(float(cfg.opts.request_timeout_ms)),
    );
    opts.insert(
        "opt.stall_threshold_ms".into(),
        Value::Float(cfg.opts.stall_threshold_ms),
    );
    let bundle = Bundle::create(
        &cfg.runs_dir,
        &pf,
        &cfg.build,
        RunSpec {
            seed: hyp.seed,
            mode: cfg.mode,
            // No scenario: the harness calls its endpoint directly (ADR-17).
            scenario_hash: Digest::ZERO,
            workload_hash: workload.hash,
            hypothesis: hyp.reference.clone(),
            params: RunParams {
                backend: cfg.backend.as_str().into(),
                model: cfg.model.clone(),
                hyp_status: hyp.status,
                arms: vec![cfg.arm.clone()],
                replicates: cfg.replicates,
                vary,
                opts,
            },
            endpoint_host: host,
            execution_order: live
                .then(|| order.iter().map(|i| format!("{}/{i}", cfg.arm)).collect()),
            started_at: live.then(acn_emu::clock::wall_time_utc),
        },
    )?;
    let dir = bundle.dir().to_path_buf();
    let result = async {
        let run_id = Digest::from_hex(bundle.run_id())?;
        let session_attrs = session_attrs(cfg, &hyp, bundle.run_id(), &knobs, &workload.hash);
        let setup = Setup {
            workload,
            knobs,
            backend: cfg.backend,
            model: cfg.model.clone(),
            opts: cfg.opts.clone(),
            inv: acn_trace::schema::inventory()?,
            counting,
            session_attrs,
        };
        let build_hash = Digest::from_hex(&cfg.build.build_hash)?;
        let collector = Collector::new();
        let seed = i64::try_from(hyp.seed)
            .map_err(|_| HarnessError::Config("the seed exceeds 2^63 - 1".into()))?;
        for &i in &order {
            let rseed = acn_trace::identity::replicate_seed(hyp.seed, i)?;
            let marker = isolation_marker(&run_id, &cfg.arm, i)?;
            let provider = SdkTracerProvider::builder()
                .with_id_generator(acn_trace::ids::SeededIdGenerator::for_replicate(
                    hyp.seed, i,
                )?)
                .with_resource(producer_resource(
                    "acn-harness",
                    env!("CARGO_PKG_VERSION"),
                    &pf.engine_hash(),
                    &build_hash,
                ))
                .with_simple_exporter(collector.exporter())
                .build();
            let tracer = provider.tracer("acn-harness");
            let streams = std::cell::RefCell::new(Streams::new(rseed)?);
            let tasks = setup.workload.tasks.len();
            if live {
                let env = LiveEnv::new(Arc::clone(&clock), &url, headers(cfg.backend, &marker)?)?;
                let rep = Replicate {
                    setup: &setup,
                    env: &env,
                    tracer: &tracer,
                    marker,
                    replicate: i,
                    seed: rseed,
                    streams,
                };
                sessions(&rep, tasks, seed).await?;
            } else {
                let mock = Mock::with_profiles(profiles.clone(), rseed)
                    .map_err(|e| HarnessError::Config(e.to_string()))?;
                let env = SimEnv::new(mock, marker.clone());
                let rep = Replicate {
                    setup: &setup,
                    env: &env,
                    tracer: &tracer,
                    marker,
                    replicate: i,
                    seed: rseed,
                    streams,
                };
                env.drive(sessions(&rep, tasks, seed))??;
            }
            provider
                .shutdown()
                .map_err(|e| HarnessError::Internal(format!("tracer: {e}")))?;
        }
        let trace = collector.trace()?;
        Ok::<_, HarnessError>(bundle.finish(&trace)?)
    }
    .await;
    if result.is_err() {
        // HAR-23 and CON-29: a run that did not finish leaves no bundle behind;
        // the directory is this run's own, created above.
        let _ = std::fs::remove_dir_all(&dir);
    }
    result
}

/// Type each `--vary` value (CON-27(c)): by its `[varies]` kind with a hypothesis,
/// where only its parameters are accepted, and by the knob's type without one,
/// where only knobs are (HAR-10).
pub fn typed_vary(
    raw: &BTreeMap<String, String>,
    varies: Option<&BTreeMap<String, Domain>>,
) -> Result<BTreeMap<String, Value>, HarnessError> {
    let mut out = BTreeMap::new();
    for (name, text) in raw {
        let domain = match varies {
            Some(v) => v.get(name).cloned().ok_or_else(|| {
                HarnessError::Knob(format!(
                    "`{name}` is not a [varies] parameter of the hypothesis"
                ))
            })?,
            None => Knobs::domain(name)
                .ok_or_else(|| HarnessError::Knob(format!("`{name}` is not a knob (HAR-10)")))?,
        };
        // HYP-6: a value outside the declared domain is refused before the run.
        out.insert(name.clone(), domain.value(name, text)?);
    }
    Ok(out)
}

async fn sessions<E: Env>(
    rep: &Replicate<'_, E>,
    tasks: usize,
    seed: i64,
) -> Result<(), HarnessError> {
    for t in 0..tasks {
        rep.session(t, seed).await?;
    }
    Ok(())
}

fn int(v: u64) -> i64 {
    i64::try_from(v).unwrap_or(i64::MAX)
}

/// A whole number of milliseconds as the float the `_ms` options are (TRC-20).
fn float(v: u64) -> f64 {
    #[allow(clippy::cast_precision_loss)] // exact below 2^53 ms, ~285 000 years
    let f = v as f64;
    f
}

/// The run-level `acn.session` attributes of TRC-10 and of the options (CON-29).
fn session_attrs(
    cfg: &RunConfig,
    hyp: &Hyp,
    run_id: &str,
    knobs: &Knobs,
    workload: &Digest,
) -> Vec<KeyValue> {
    vec![
        KeyValue::new("acn.run_id", run_id.to_owned()),
        KeyValue::new("acn.hypothesis.id", hyp.reference.id.clone()),
        KeyValue::new("acn.hypothesis.status", hyp.status.as_str()),
        KeyValue::new("acn.backend", cfg.backend.as_str()),
        KeyValue::new("acn.mode", cfg.mode.as_str()),
        KeyValue::new("acn.scenario.hash", Digest::ZERO.to_hex()),
        KeyValue::new("acn.workload.hash", workload.to_hex()),
        KeyValue::new("acn.role", cfg.arm.clone()),
        KeyValue::new("acn.harness.knobs", knobs.to_json()),
        KeyValue::new("acn.stall_threshold_ms", cfg.opts.stall_threshold_ms),
        KeyValue::new("acn.keep_content", false),
        KeyValue::new("acn.harness.endpoint", cfg.opts.endpoint.clone()),
        KeyValue::new("acn.harness.max_retries", int(cfg.opts.max_retries)),
        KeyValue::new("acn.harness.retry_base_ms", float(cfg.opts.retry_base_ms)),
        KeyValue::new(
            "acn.harness.request_timeout_ms",
            float(cfg.opts.request_timeout_ms),
        ),
    ]
}
