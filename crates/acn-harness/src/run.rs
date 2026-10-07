//! One run (HAR-50..52): one cell and one arm of a workload, every replicate in
//! its seeded order (HAR-43), into one bundle.

use std::collections::{BTreeMap, BTreeSet};
use std::path::{Path, PathBuf};
use std::sync::Arc;

use acn_emu::clock::Clock as _;
use acn_mockllm::Mock;
use acn_mockllm::profile::Profiles;
use acn_trace::bundle::{Bundle, HypothesisRef, RunSpec, Written};
use acn_trace::env::RunHypothesis;
use acn_trace::identity::{BuildInfo, Digest, HypStatus, Mode, Preimage, RunParams, Value};
use acn_trace::otel::{Collector, producer_resource};
use opentelemetry::KeyValue;
use opentelemetry::trace::{Span as _, SpanKind, Tracer as _, TracerProvider as _};
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
    // HAR-26: the mock the harness serves itself, a fresh one per replicate.
    if opts.endpoint == crate::served::LOOPBACK {
        if backend != Backend::Mockllm {
            return Err(HarnessError::Config(format!(
                "{} serves the mock, not `{}` (HAR-26)",
                crate::served::LOOPBACK,
                backend.as_str()
            )));
        }
        return Ok((String::new(), crate::served::LOOPBACK_HOST.to_owned()));
    }
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

/// HAR-23: whether the endpoint at `url` is the backend the run was
/// configured with.
async fn probe(
    client: &reqwest::Client,
    backend: Backend,
    clock: &Arc<acn_emu::clock::WallClock>,
    url: &str,
) -> Result<(), HarnessError> {
    let probe = LiveEnv::with_client(
        client.clone(),
        Arc::clone(clock),
        url,
        headers(backend, "-")?,
    );
    let is_mock = probe.probe_is_mock().await?;
    if is_mock != (backend == Backend::Mockllm) {
        return Err(HarnessError::BackendMismatch(format!(
            "configured `{}`, but the endpoint {} the mock's marker (CON-26, HAR-23)",
            backend.as_str(),
            if is_mock { "shows" } else { "does not show" }
        )));
    }
    Ok(())
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
    run_with_scenario(cfg, None)
}

/// [`run`] with the calls of a `sim` run crossing a scenario's network
/// (SPEC 020 §4); `None` is [`run`].
pub fn run_with_scenario(
    cfg: &RunConfig,
    scenario: Option<&Path>,
) -> Result<Written, HarnessError> {
    run_driven_blocking(cfg, scenario, &AgentDriver)
}

/// [`run`] on the caller's runtime. The future is not `Send`: the agent's
/// lineages share their state through `RefCell`s, so it is awaited in place (or
/// on a `LocalSet`), never spawned onto another thread.
pub async fn run_async(cfg: &RunConfig) -> Result<Written, HarnessError> {
    run_async_with(cfg, None).await
}

/// A scenario as a run uses it: its links, the file's bytes and hash.
struct RunScenario {
    scenario: acn_emu::scenario::Scenario,
    toml: String,
    hash: Digest,
}

/// What decides a run's sessions (SPEC 050 GEN-20). The run path does
/// everything else: identity, environments, network, proxy, served mock,
/// retries, spans and the bundle. The agent loop over a workload file
/// (HAR-1) is [`AgentDriver`]; another crate's driver plugs in here, and the
/// harness never depends on it (ADR-38).
pub trait Driver {
    /// The workload the run is identified by, whose hash enters `run_id`
    /// (CON-29), and whose `[agent]` settings every call uses. A workload
    /// built in memory sets `hash` to the BLAKE3 of the file it stands for
    /// (CON-27(a)): nothing else checks it.
    fn workload(&self, cfg: &RunConfig) -> Result<Workload, HarnessError>;
    /// The `service.name` and `service.version` of the run's spans (TRC-19).
    fn producer(&self) -> (&'static str, &'static str);
    /// Whether every knob must stay at its default (SPEC 050 GEN-21).
    fn knobs_fixed(&self) -> bool {
        false
    }
    /// One replicate's sessions, `seed` being the run seed (`acn.seed`).
    fn replicate<E: Env>(
        &self,
        rep: &Replicate<'_, E>,
        seed: i64,
    ) -> impl std::future::Future<Output = Result<(), HarnessError>>;
}

/// The agent loop over a workload file (HAR-1).
#[derive(Debug, Clone, Copy, Default)]
pub struct AgentDriver;

impl Driver for AgentDriver {
    fn workload(&self, cfg: &RunConfig) -> Result<Workload, HarnessError> {
        Workload::load(&cfg.workload)
    }
    fn producer(&self) -> (&'static str, &'static str) {
        ("acn-harness", env!("CARGO_PKG_VERSION"))
    }
    async fn replicate<E: Env>(
        &self,
        rep: &Replicate<'_, E>,
        seed: i64,
    ) -> Result<(), HarnessError> {
        sessions(rep, rep.setup.workload.tasks.len(), seed).await
    }
}

/// [`run_with_scenario`] on the caller's runtime.
pub async fn run_async_with(
    cfg: &RunConfig,
    scenario: Option<&Path>,
) -> Result<Written, HarnessError> {
    run_driven(cfg, scenario, &AgentDriver).await
}

/// [`run_driven`] on a runtime of its own.
pub fn run_driven_blocking<D: Driver>(
    cfg: &RunConfig,
    scenario: Option<&Path>,
    driver: &D,
) -> Result<Written, HarnessError> {
    tokio::runtime::Builder::new_current_thread()
        .enable_all()
        .build()
        .map_err(|e| HarnessError::Internal(format!("runtime: {e}")))?
        .block_on(run_driven(cfg, scenario, driver))
}

/// Everything a run decides before it touches the network or its runs
/// directory: its inputs, its checks and the spec its identity comes from.
struct Prepared {
    scenario: Option<RunScenario>,
    scenario_hash: Digest,
    workload: Workload,
    hyp: Hyp,
    knobs: Knobs,
    profiles: Profiles,
    counting: Counting,
    pf: acn_trace::env::Preflight,
    live: bool,
    served: bool,
    url: String,
    order: Vec<u32>,
    spec: RunSpec,
}

/// A run's identity, known before it runs (CON-29; SPEC 070 CTL-13).
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct Planned {
    pub run_id: Digest,
    /// `runs_dir/<run_id>/`.
    pub dir: PathBuf,
    pub workload_hash: Digest,
    pub scenario_hash: Digest,
    pub hypothesis_hash: Digest,
}

/// The checks, inputs and spec of a run, shared by [`plan_driven`] and
/// [`run_driven`] so that a plan's `run_id` is the run's.
fn prepare<D: Driver>(
    cfg: &RunConfig,
    scenario: Option<&Path>,
    driver: &D,
) -> Result<Prepared, HarnessError> {
    let (producer, _) = driver.producer();
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
    let scenario = match scenario {
        None => None,
        Some(path) => {
            let sc = acn_emu::scenario::load(path)
                .map_err(|e| HarnessError::Config(format!("scenario: {e}")))?;
            let toml = std::fs::read_to_string(path)
                .map_err(|e| HarnessError::Config(format!("scenario: {e}")))?;
            // The text recorded in the scenario span is the text the hash names.
            if blake3::hash(toml.as_bytes()).to_hex().as_str() != sc.hash {
                return Err(HarnessError::Config(
                    "scenario: the file changed while it was read".into(),
                ));
            }
            let names: BTreeSet<&str> = sc.links.iter().map(|l| l.name.as_str()).collect();
            if names.len() != 1 {
                return Err(HarnessError::Config(format!(
                    "scenario: a run's scenario has exactly one path, not {} (SPEC 020 EMU-32)",
                    names.len()
                )));
            }
            let hash = Digest::from_hex(&sc.hash)?;
            Some(RunScenario {
                scenario: sc,
                toml,
                hash,
            })
        }
    };
    let scenario_hash = scenario.as_ref().map_or(Digest::ZERO, |s| s.hash);
    let workload = driver.workload(cfg)?;
    let hyp = hypothesis(&cfg.hypothesis, &cfg.start_dir)?;
    let vary = typed_vary(&cfg.vary, hyp.varies.as_ref())?;
    let knobs = Knobs::from_vary(&vary)?;
    if driver.knobs_fixed() && knobs != Knobs::default() {
        return Err(HarnessError::Knob(format!(
            "`{producer}` runs every knob at its default; a knob `vary` is refused (SPEC 050 GEN-21)"
        )));
    }
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
    let served = cfg.opts.endpoint == crate::served::LOOPBACK;
    if served && !live {
        return Err(HarnessError::Config(format!(
            "{} is a `live` endpoint; `sim` calls the mock in process (HAR-26)",
            crate::served::LOOPBACK
        )));
    }
    let (url, host) = if live {
        let (u, h) = endpoint(cfg.backend, &cfg.opts)?;
        (u, Some(h))
    } else {
        (String::new(), None)
    };
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
    let spec = RunSpec {
        seed: hyp.seed,
        mode: cfg.mode,
        // No scenario: the harness calls its endpoint directly (ADR-17,
        // SPEC 020 EMU-39); with one, its calls cross the network (ADR-34).
        scenario_hash,
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
        execution_order: live.then(|| order.iter().map(|i| format!("{}/{i}", cfg.arm)).collect()),
        // Set when the run starts: it is not part of the identity.
        started_at: None,
    };
    Ok(Prepared {
        scenario,
        scenario_hash,
        workload,
        hyp,
        knobs,
        profiles,
        counting,
        pf,
        live,
        served,
        url,
        order,
        spec,
    })
}

/// The identity a run of `cfg` would have, with nothing written and no
/// network touched (SPEC 070 CTL-13). The endpoint is not probed (HAR-23):
/// that happens when the run starts.
pub fn plan_driven<D: Driver>(
    cfg: &RunConfig,
    scenario: Option<&Path>,
    driver: &D,
) -> Result<Planned, HarnessError> {
    let p = prepare(cfg, scenario, driver)?;
    let mut spec = p.spec;
    // A `live` manifest must carry a start time to validate; it is not part
    // of the identity.
    spec.started_at = p.live.then(acn_emu::clock::wall_time_utc);
    let hypothesis_hash = spec.hypothesis.hash;
    let plan = Bundle::plan(&p.pf, &cfg.build, spec)?;
    let run_id = Digest::from_hex(plan.run_id())?;
    Ok(Planned {
        run_id,
        dir: cfg.runs_dir.join(run_id.to_hex()),
        workload_hash: p.workload.hash,
        scenario_hash: p.scenario_hash,
        hypothesis_hash,
    })
}

/// [`plan_driven`] for the agent loop.
pub fn plan(cfg: &RunConfig, scenario: Option<&Path>) -> Result<Planned, HarnessError> {
    plan_driven(cfg, scenario, &AgentDriver)
}

/// One cell and arm into one bundle, its sessions decided by `driver`
/// (SPEC 050 GEN-20): the run path of HAR-50 with the driver's workload,
/// producer and sessions.
pub async fn run_driven<D: Driver>(
    cfg: &RunConfig,
    scenario: Option<&Path>,
    driver: &D,
) -> Result<Written, HarnessError> {
    let (producer, producer_version) = driver.producer();
    let Prepared {
        scenario,
        scenario_hash,
        workload,
        hyp,
        knobs,
        profiles,
        counting,
        pf,
        live,
        served,
        url,
        order,
        mut spec,
    } = prepare(cfg, scenario, driver)?;
    let clock = Arc::new(acn_emu::clock::WallClock::start());
    // One HTTP client for the run's live calls: building one is slow.
    let live_client = if live {
        Some(LiveEnv::http_client()?)
    } else {
        None
    };
    if let (Some(client), false) = (&live_client, served) {
        // HAR-23: the endpoint says what it is before the run gets an identity.
        // A served mock is probed per replicate instead, once it exists (HAR-26).
        probe(client, cfg.backend, &clock, &url).await?;
    }

    spec.started_at = live.then(acn_emu::clock::wall_time_utc);
    let bundle = Bundle::plan(&pf, &cfg.build, spec)?.create(&cfg.runs_dir)?;
    let dir = bundle.dir().to_path_buf();
    let result = async {
        let run_id = Digest::from_hex(bundle.run_id())?;
        let session_attrs = session_attrs(
            cfg,
            &hyp,
            bundle.run_id(),
            &knobs,
            &workload.hash,
            &scenario_hash,
        );
        // Only the attributes SPEC 010 lists for this producer (TRC-19,
        // SPEC 050 GEN-21): the harness's own list is unchanged.
        let inv = acn_trace::schema::inventory()?;
        // The inventory lists every attribute (TRC-20): one it does not know
        // is the harness's fault, not a key to keep or drop silently.
        let mut kept = Vec::with_capacity(session_attrs.len());
        for kv in session_attrs {
            let Some(a) = inv.attribute(kv.key.as_str()) else {
                return Err(HarnessError::Internal(format!(
                    "`{}` is not in the attribute inventory (TRC-20)",
                    kv.key
                )));
            };
            if a.producers.iter().any(|p| p == producer) {
                kept.push(kv);
            }
        }
        let session_attrs = kept;
        let setup = Setup {
            workload,
            knobs,
            backend: cfg.backend,
            model: cfg.model.clone(),
            opts: cfg.opts.clone(),
            inv,
            counting,
            session_attrs,
        };
        let build_hash = Digest::from_hex(&cfg.build.build_hash)?;
        let collector = Collector::new();
        let seed = i64::try_from(hyp.seed)
            .map_err(|_| HarnessError::Config("the seed exceeds 2^63 - 1".into()))?;
        let emu_resource = || {
            producer_resource(
                "acn-emu",
                env!("CARGO_PKG_VERSION"),
                &pf.engine_hash(),
                &build_hash,
            )
        };
        // EMU-37: the run's scenario span, its ids drawn from the run's stream
        // before the first replicate, so every link span can link to it.
        let scenario_run = match &scenario {
            None => None,
            Some(sc) => {
                let provider = SdkTracerProvider::builder()
                    .with_id_generator(acn_trace::ids::SeededIdGenerator::for_run(hyp.seed)?)
                    .with_resource(emu_resource())
                    .with_max_events_per_span(u32::MAX)
                    .with_simple_exporter(collector.exporter())
                    .build();
                let tracer = provider.tracer("acn-emu");
                let span = tracer
                    .span_builder("acn.scenario")
                    .with_kind(SpanKind::Internal)
                    .with_start_time(crate::agent::at_ns(0))
                    .with_attributes(vec![
                        KeyValue::new("acn.scenario.toml", sc.toml.clone()),
                        KeyValue::new("acn.scenario.hash", sc.hash.to_hex()),
                    ])
                    .start(&tracer);
                Some((provider, span))
            }
        };
        let scenario_cx = scenario_run
            .as_ref()
            .map(|(_, span)| span.span_context().clone());
        let mut scenario_log = ScenarioLog::default();
        for &i in &order {
            let rseed = acn_trace::identity::replicate_seed(hyp.seed, i)?;
            let marker = isolation_marker(&run_id, &cfg.arm, i)?;
            // HAR-26: the replicate's own mock, built as in `sim`, served until
            // the replicate ends.
            let mut mock_server = match (&live_client, served) {
                (Some(client), true) => {
                    let mock = Mock::with_profiles(profiles.clone(), rseed)
                        .map_err(|e| HarnessError::Config(e.to_string()))?;
                    let s = crate::served::ServedMock::start(
                        mock,
                        Arc::clone(&clock) as Arc<dyn acn_emu::clock::Clock>,
                    )
                    .await?;
                    // HAR-23 against the replicate's server. It is the mock by
                    // construction, so this checks that it answers; the probe
                    // reads only the profiles, so the mock stays as built.
                    probe(client, cfg.backend, &clock, &s.url()).await?;
                    Some(s)
                }
                _ => None,
            };
            let url = mock_server
                .as_ref()
                .map_or_else(|| url.clone(), |s| s.url());
            let harness_resource =
                producer_resource(producer, producer_version, &pf.engine_hash(), &build_hash);
            let streams = std::cell::RefCell::new(Streams::new(rseed)?);
            if let (Some(sc), Some(scenario_cx)) = (&scenario, &scenario_cx) {
                // EMU-36: the harness's and the network's spans draw from one
                // replicate stream, in program order.
                let ids = Arc::new(acn_trace::ids::SeededIdGenerator::for_replicate(
                    hyp.seed, i,
                )?);
                let provider = SdkTracerProvider::builder()
                    .with_id_generator(acn_trace::ids::SharedIdGenerator(Arc::clone(&ids)))
                    .with_resource(harness_resource)
                    .with_simple_exporter(collector.exporter())
                    .build();
                let emu = SdkTracerProvider::builder()
                    .with_id_generator(acn_trace::ids::SharedIdGenerator(ids))
                    .with_resource(emu_resource())
                    .with_simple_exporter(collector.exporter())
                    .build();
                let tracer = provider.tracer(producer);
                let emu_tracer = emu.tracer("acn-emu");
                let model = |d| {
                    sc.scenario
                        .links
                        .iter()
                        .find(|l| l.direction == d)
                        .map_or_else(|| "none".to_owned(), acn_emu::link::LinkSpec::model_name)
                };
                let mut net = crate::agent::NetSpans {
                    tracer: &emu_tracer,
                    scenario: scenario_cx.clone(),
                    link_id: sc
                        .scenario
                        .links
                        .first()
                        .map(|l| l.name.clone())
                        .unwrap_or_default(),
                    up_model: model(acn_emu::link::Direction::Up),
                    down_model: model(acn_emu::link::Direction::Down),
                    // In `sim` every replicate starts at 0; in `live` the
                    // origin is read just before the proxy starts (EMU-40).
                    origin_ns: 0,
                };
                if live {
                    // EMU-40: the replicate's own proxy, its links built from
                    // the replicate seed before it takes a connection.
                    let mut links = sc
                        .scenario
                        .build(rseed)
                        .map_err(|e| HarnessError::Config(format!("scenario: {e}")))?;
                    let up_at = links
                        .iter()
                        .position(|l| l.spec().direction == acn_emu::link::Direction::Up);
                    let (up, down) = match up_at {
                        Some(k) => {
                            let up = links.remove(k);
                            (up, links.pop())
                        }
                        None => (links.remove(0), None),
                    };
                    let Some(down) = down else {
                        return Err(HarnessError::Config(
                            "scenario: a path needs an up and a down link".into(),
                        ));
                    };
                    let headers = headers(cfg.backend, &marker)?;
                    let client = live_client.clone().ok_or_else(|| {
                        HarnessError::Internal("a live run without its client".into())
                    })?;
                    // The links count from the replicate's start on the run's
                    // clock, read as late as possible: everything slow is done,
                    // and the sessions start next (EMU-40).
                    let origin_ns = clock.now_ns();
                    net.origin_ns = origin_ns;
                    let proxy = Arc::new(
                        acn_emu::proxy::Proxy::start(
                            up,
                            down,
                            &url,
                            Arc::clone(&clock) as Arc<dyn acn_emu::clock::Clock>,
                            origin_ns,
                        )
                        .await
                        .map_err(|e| HarnessError::Config(format!("proxy: {e}")))?,
                    );
                    let env = LiveEnv::through_proxy(
                        client,
                        Arc::clone(&clock),
                        Arc::clone(&proxy),
                        origin_ns,
                        headers,
                    );
                    let rep = Replicate {
                        setup: &setup,
                        env: &env,
                        tracer: &tracer,
                        net: Some(net),
                        marker,
                        replicate: i,
                        seed: rseed,
                        streams,
                    };
                    let ran = driver.replicate(&rep, seed).await;
                    let ended = clock.now_ns();
                    proxy.shutdown().await;
                    // A proxy fault explains whatever the sessions saw: it
                    // is reported first (EMU-40).
                    if let Some(f) = proxy.fault() {
                        return Err(HarnessError::Internal(format!("the live proxy: {f}")));
                    }
                    ran?;
                    // After the proxy: no forward can still reach the mock.
                    if let Some(s) = mock_server.take() {
                        s.shutdown().await?;
                    }
                    scenario_log.record(&sc.scenario, &proxy.fates(), ended, origin_ns);
                } else {
                    let mock = Mock::with_profiles(profiles.clone(), rseed)
                        .map_err(|e| HarnessError::Config(e.to_string()))?;
                    let env = SimEnv::with_scenario(mock, marker.clone(), &sc.scenario, rseed)?;
                    let rep = Replicate {
                        setup: &setup,
                        env: &env,
                        tracer: &tracer,
                        net: Some(net),
                        marker,
                        replicate: i,
                        seed: rseed,
                        streams,
                    };
                    env.drive(driver.replicate(&rep, seed))??;
                    scenario_log.record(&sc.scenario, &env.fates(), env.now(), 0);
                }
                for p in [provider, emu] {
                    p.shutdown()
                        .map_err(|e| HarnessError::Internal(format!("tracer: {e}")))?;
                }
                continue;
            }
            let provider = SdkTracerProvider::builder()
                .with_id_generator(acn_trace::ids::SeededIdGenerator::for_replicate(
                    hyp.seed, i,
                )?)
                .with_resource(harness_resource)
                .with_simple_exporter(collector.exporter())
                .build();
            let tracer = provider.tracer(producer);
            if live {
                let client = live_client.clone().ok_or_else(|| {
                    HarnessError::Internal("a live run without its client".into())
                })?;
                let env = LiveEnv::with_client(
                    client,
                    Arc::clone(&clock),
                    &url,
                    headers(cfg.backend, &marker)?,
                );
                let rep = Replicate {
                    setup: &setup,
                    env: &env,
                    tracer: &tracer,
                    net: None,
                    marker,
                    replicate: i,
                    seed: rseed,
                    streams,
                };
                driver.replicate(&rep, seed).await?;
                if let Some(s) = mock_server.take() {
                    s.shutdown().await?;
                }
            } else {
                let mock = Mock::with_profiles(profiles.clone(), rseed)
                    .map_err(|e| HarnessError::Config(e.to_string()))?;
                let env = SimEnv::new(mock, marker.clone());
                let rep = Replicate {
                    setup: &setup,
                    env: &env,
                    tracer: &tracer,
                    net: None,
                    marker,
                    replicate: i,
                    seed: rseed,
                    streams,
                };
                env.drive(driver.replicate(&rep, seed))??;
            }
            provider
                .shutdown()
                .map_err(|e| HarnessError::Internal(format!("tracer: {e}")))?;
        }
        if let Some((provider, mut span)) = scenario_run {
            scenario_log.emit(&mut span);
            span.end_with_timestamp(crate::agent::at_ns(scenario_log.end_ns));
            drop(span);
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
    scenario: &Digest,
) -> Vec<KeyValue> {
    vec![
        KeyValue::new("acn.run_id", run_id.to_owned()),
        KeyValue::new("acn.hypothesis.id", hyp.reference.id.clone()),
        KeyValue::new("acn.hypothesis.status", hyp.status.as_str()),
        KeyValue::new("acn.backend", cfg.backend.as_str()),
        KeyValue::new("acn.mode", cfg.mode.as_str()),
        KeyValue::new("acn.scenario.hash", scenario.to_hex()),
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

/// The events of the run's `acn.scenario` span (SPEC 020 EMU-37), gathered
/// from every replicate's fates: one outage event per window in which a message
/// was dropped or held, and per trace outage segment in which one was dropped;
/// one step event the first time a message is sent in each trace segment.
/// Replicates share the scenario's timeline, so an event met by several is
/// recorded once.
/// Where an event of the scenario span sorts: time, link name, direction,
/// kind (outage before step) and window or sample.
type EventKey = (i64, String, u8, u8, usize);

/// An event of the scenario span: its name and fields.
type ScenarioEvent = (&'static str, Vec<KeyValue>);

#[derive(Default)]
struct ScenarioLog {
    /// The latest end of any replicate's sessions.
    end_ns: i64,
    /// By (time, link name, direction, kind, window or sample): the event.
    events: BTreeMap<EventKey, ScenarioEvent>,
    /// Trace segment occurrences stepped into: (link, direction, sample,
    /// occurrence start) to the first send time in it and the segment's
    /// values as JSON. A sample recurs once per period of the trace.
    stepped: BTreeMap<(String, u8, usize, i64), (i64, String)>,
}

impl ScenarioLog {
    fn record(
        &mut self,
        scenario: &acn_emu::scenario::Scenario,
        fates: &[(acn_emu::link::Direction, acn_emu::link::Fate)],
        end_ns: i64,
        origin_ns: i64,
    ) {
        use acn_emu::link::{Direction, DropCause, OutageCause};
        self.end_ns = self.end_ns.max(end_ns);
        for (dir, f) in fates {
            let Some(link) = scenario.links.iter().find(|l| l.direction == *dir) else {
                continue;
            };
            let d = match dir {
                Direction::Up => 0,
                Direction::Down => 1,
            };
            let dropped_by_outage = f.outcome == Err(DropCause::Outage);
            if let (Some(ws), Some(w)) = (&link.outage, f.window)
                && (f.hold_ns > 0 || dropped_by_outage)
                && let Some(win) = ws.get(w)
            {
                let cause = match win.cause {
                    OutageCause::Handover => "handover",
                    OutageCause::Scheduled => "scheduled",
                };
                // On the run's clock: the replicate's origin plus link time
                // (EMU-47); the origin is 0 in `sim`.
                let (a, b) = (origin_ns + win.start_ns, origin_ns + win.end_ns);
                self.events.insert(
                    (a, link.name.clone(), d, 0, w),
                    (
                        "acn.scenario.outage",
                        vec![
                            KeyValue::new("start_ns", a),
                            KeyValue::new("end_ns", b),
                            KeyValue::new("cause", cause),
                        ],
                    ),
                );
            }
            if let (Some(tr), Some(k)) = (&link.trace, f.sample) {
                let (_, start, end) = tr.segment_at(f.send_ns);
                // The first occurrence can begin before the replicate does.
                let (start, end) = (origin_ns + start.max(0), origin_ns + end);
                if dropped_by_outage && tr.segments.get(k).is_some_and(|s| s.outage) {
                    self.events.insert(
                        (start, link.name.clone(), d, 0, k),
                        (
                            "acn.scenario.outage",
                            vec![
                                KeyValue::new("start_ns", start),
                                KeyValue::new("end_ns", end),
                                KeyValue::new("cause", "trace"),
                            ],
                        ),
                    );
                }
                if let Some(seg) = tr.segments.get(k) {
                    let params = format!(
                        "{{\"sample\":{k},\"loss_ppm\":{},\"rate_bps\":{},\"delay_ns\":{},\"jitter_ns\":{},\"outage\":{}}}",
                        seg.loss_ppm, seg.rate_bps, seg.delay_ns, seg.jitter_ns, seg.outage
                    );
                    let first = self
                        .stepped
                        .entry((link.name.clone(), d, k, start))
                        .or_insert((origin_ns + f.send_ns, params));
                    first.0 = first.0.min(origin_ns + f.send_ns);
                }
            }
        }
    }

    /// Add the events to the scenario span, in order (EMU-37).
    fn emit(&mut self, span: &mut opentelemetry_sdk::trace::Span) {
        for ((link, d, k, _), (t, params)) in std::mem::take(&mut self.stepped) {
            let dir = if d == 0 { "up" } else { "down" };
            self.events.insert(
                (t, link.clone(), d, 1, k),
                (
                    "acn.scenario.step",
                    vec![
                        KeyValue::new("step", format!("{link}.{dir}.{k}")),
                        KeyValue::new("params", params),
                    ],
                ),
            );
        }
        for ((t, ..), (name, attrs)) in std::mem::take(&mut self.events) {
            span.add_event_with_timestamp(name, crate::agent::at_ns(t), attrs);
        }
    }
}
