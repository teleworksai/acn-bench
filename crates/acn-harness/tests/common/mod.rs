//! Helpers shared by the harness's tests: test mock profiles with small
//! constants, one session against the in-process mock with every request body
//! recorded, and span lookups.
#![allow(
    dead_code,
    clippy::unwrap_used,
    clippy::expect_used,
    clippy::panic,
    clippy::missing_panics_doc
)]

use std::cell::RefCell;
use std::collections::BTreeMap;

use acn_harness::agent::{Counting, Opts, Replicate, Setup, Streams};
use acn_harness::env::{Env, SimEnv};
use acn_harness::knobs::Knobs;
use acn_harness::run::typed_vary;
use acn_harness::wire::{Backend, Exchange};
use acn_harness::workload::Workload;
use acn_mockllm::Mock;
use acn_mockllm::profile::Profiles;
use acn_trace::model::{AttrValue, SpanRow, Trace};
use acn_trace::otel::{Collector, producer_resource};
use opentelemetry::KeyValue;
use opentelemetry::trace::TracerProvider as _;
use opentelemetry_sdk::trace::SdkTracerProvider;
use serde_json::Value;

/// The smoke workload's text (HAR-61).
pub fn smoke() -> String {
    std::fs::read_to_string(concat!(
        env!("CARGO_MANIFEST_DIR"),
        "/../../workloads/harness-smoke.toml"
    ))
    .unwrap()
}

/// A mock profile with small constants; `overrides` are `key = value` lines.
pub fn profile(name: &str, cache_model: &str, overrides: &[&str]) -> String {
    let mut fields = vec![
        ("placeholder", "true".to_owned()),
        ("doc", "\"test\"".to_owned()),
        ("cache_model", format!("\"{cache_model}\"")),
        (
            "prefix_order",
            "[\"tools\", \"system\", \"messages\"]".to_owned(),
        ),
        ("ttl_ns", "1000000000000".to_owned()),
        ("min_cacheable_tokens", "32".to_owned()),
        ("increment_tokens", "16".to_owned()),
        ("max_breakpoints", "4".to_owned()),
        ("block_tokens", "16".to_owned()),
        ("capacity_blocks", "100000".to_owned()),
        ("slots", "0".to_owned()),
        ("prefill_base_ns", "1000000".to_owned()),
        ("prefill_ns_per_new_token", "1000".to_owned()),
        ("prefill_ns_per_cached_token", "100".to_owned()),
        ("itl_ns", "1000000".to_owned()),
        ("itl_jitter_ns", "0".to_owned()),
        ("tool_calls_per_turn", "1".to_owned()),
        ("output_tokens_min", "8".to_owned()),
        ("output_tokens_max", "8".to_owned()),
        ("fault_429_ppm", "0".to_owned()),
        ("fault_500_ppm", "0".to_owned()),
        ("fault_cut_ppm", "0".to_owned()),
        ("retry_after_s_max", "2".to_owned()),
    ];
    for o in overrides {
        let (k, v) = o.split_once('=').unwrap();
        let slot = fields.iter_mut().find(|(f, _)| *f == k.trim()).unwrap();
        slot.1 = v.trim().to_owned();
    }
    let mut s = format!("[[profile]]\nname = \"{name}\"\n");
    for (k, v) in fields {
        s.push_str(&format!("{k} = {v}\n"));
    }
    s
}

/// A profiles file of the given profiles.
pub fn profiles(list: &[String]) -> Profiles {
    Profiles::parse(&format!("schema_version = 1\n\n{}", list.join("\n"))).unwrap()
}

/// The three test profiles, one per cache model, as `explicit`, `auto` and `blocks`.
pub fn three() -> Profiles {
    profiles(&[
        profile("explicit", "explicit_breakpoints", &[]),
        profile("auto", "automatic_prefix", &[]),
        profile("blocks", "block_granular", &[]),
    ])
}

/// An [`Env`] that records every request body it is given.
pub struct Recording<E> {
    pub inner: E,
    pub bodies: RefCell<Vec<(i64, Vec<u8>)>>,
}

impl<E: Env> Env for Recording<E> {
    fn now(&self) -> i64 {
        self.inner.now()
    }

    async fn sleep_until(&self, t_ns: i64) {
        self.inner.sleep_until(t_ns).await;
    }

    async fn exchange(
        &self,
        path: &'static str,
        body: Vec<u8>,
        stream: bool,
        timeout_ns: i64,
    ) -> Exchange {
        self.bodies.borrow_mut().push((self.now(), body.clone()));
        self.inner.exchange(path, body, stream, timeout_ns).await
    }
}

/// What one session left behind.
pub struct Ran {
    pub trace: Trace,
    /// Every request body, parsed, with the time it was sent.
    pub bodies: Vec<(i64, Value)>,
    pub cache: (usize, usize),
    pub result: Result<(), acn_harness::HarnessError>,
}

/// The marker every helper session uses, so that two sessions differ only in
/// what the test varies (HAR-17).
pub const MARKER: &str = "0123456789abcdef";

/// How one helper session is set up.
pub struct Spec<'a> {
    pub workload: String,
    pub task: usize,
    pub model: &'a str,
    pub profiles: Profiles,
    pub vary: &'a [(&'a str, &'a str)],
    pub seed: u64,
    pub opts: Opts,
}

impl Default for Spec<'_> {
    fn default() -> Self {
        Self {
            workload: smoke(),
            task: 0,
            model: "auto",
            profiles: three(),
            vary: &[],
            seed: 1,
            opts: Opts::default(),
        }
    }
}

/// Run one session of `spec` against the in-process mock.
pub fn session(spec: Spec<'_>) -> Ran {
    let workload = Workload::parse(spec.workload.as_bytes()).unwrap();
    let raw: BTreeMap<String, String> = spec
        .vary
        .iter()
        .map(|(k, v)| ((*k).to_owned(), (*v).to_owned()))
        .collect();
    let knobs = Knobs::from_vary(&typed_vary(&raw, None).unwrap()).unwrap();
    let profile = spec.profiles.get(spec.model).unwrap().clone();
    let setup = Setup {
        workload,
        knobs,
        backend: Backend::Mockllm,
        model: spec.model.to_owned(),
        opts: spec.opts,
        inv: acn_trace::schema::inventory().unwrap(),
        counting: Counting::Tokens(Box::new(profile)),
        session_attrs: vec![
            KeyValue::new("acn.role", "treatment"),
            KeyValue::new("acn.harness.knobs", knobs.to_json()),
        ],
    };
    let mock = Mock::with_profiles(spec.profiles, spec.seed).unwrap();
    let env = SimEnv::new(mock, MARKER.to_owned());
    let rec = Recording {
        inner: env.clone(),
        bodies: RefCell::new(Vec::new()),
    };
    let collector = Collector::new();
    let digest = acn_trace::identity::Digest::of(b"test");
    let provider = SdkTracerProvider::builder()
        .with_id_generator(acn_trace::ids::SeededIdGenerator::for_replicate(spec.seed, 0).unwrap())
        .with_resource(producer_resource("acn-harness", "0.1.0", &digest, &digest))
        .with_simple_exporter(collector.exporter())
        .build();
    let tracer = provider.tracer("acn-harness");
    let rep = Replicate {
        setup: &setup,
        env: &rec,
        tracer: &tracer,
        marker: MARKER.to_owned(),
        replicate: 0,
        streams: RefCell::new(Streams::new(spec.seed).unwrap()),
    };
    let result = env
        .drive(rep.session(spec.task, i64::try_from(spec.seed).unwrap()))
        .unwrap();
    provider.shutdown().unwrap();
    let bodies = rec
        .bodies
        .borrow()
        .iter()
        .map(|(t, b)| (*t, serde_json::from_slice(b).unwrap()))
        .collect();
    Ran {
        trace: collector.trace().unwrap(),
        bodies,
        cache: env.cache_sizes(),
        result,
    }
}

/// The spans named `name`, in stored order.
pub fn spans<'a>(t: &'a Trace, name: &str) -> Vec<&'a SpanRow> {
    t.spans.iter().filter(|s| s.name == name).collect()
}

pub fn int(s: &SpanRow, key: &str) -> Option<i64> {
    match s.attrs.get(key) {
        Some(AttrValue::Int(v)) => Some(*v),
        _ => None,
    }
}

pub fn text<'a>(s: &'a SpanRow, key: &str) -> Option<&'a str> {
    match s.attrs.get(key) {
        Some(AttrValue::String(v)) => Some(v),
        _ => None,
    }
}

pub fn float(s: &SpanRow, key: &str) -> Option<f64> {
    match s.attrs.get(key) {
        Some(AttrValue::Float(v)) => Some(*v),
        _ => None,
    }
}

/// The children of `parent`.
pub fn children<'a>(t: &'a Trace, parent: &SpanRow) -> Vec<&'a SpanRow> {
    t.spans
        .iter()
        .filter(|s| s.parent_span_id == Some(parent.span_id))
        .collect()
}

/// `acn.cache.read_tokens` of every chat, in start order.
pub fn cached(t: &Trace) -> Vec<i64> {
    spans(t, "chat")
        .iter()
        .map(|s| int(s, "acn.cache.read_tokens").unwrap())
        .collect()
}

/// A build identity for test runs.
pub fn build() -> acn_trace::identity::BuildInfo {
    use acn_trace::identity::{BuildParts, Digest};
    BuildParts {
        cargo_lock: Digest::of(b"lock"),
        rust_toolchain: Digest::of(b"toolchain"),
        cargo_config: Digest::of(b"config"),
        source_hash: Digest::of(b"source"),
        target: "test",
        profile: "debug",
        features: "",
        rustflags: "",
    }
    .info()
    .unwrap()
}

/// A run of `workload` (written to a temporary file) with everything else at its
/// test default: sim, the test profiles, seed 7, one replicate, no hypothesis.
pub struct RunFixture {
    pub cfg: acn_harness::run::RunConfig,
    pub dir: tempfile::TempDir,
}

pub fn run_fixture(workload: &str, model: &str) -> RunFixture {
    let dir = tempfile::tempdir().unwrap();
    let path = dir.path().join("workload.toml");
    std::fs::write(&path, workload).unwrap();
    let cfg = acn_harness::run::RunConfig {
        workload: path,
        backend: Backend::Mockllm,
        model: model.to_owned(),
        mode: acn_trace::identity::Mode::Sim,
        arm: "treatment".into(),
        replicates: 1,
        vary: BTreeMap::new(),
        opts: Opts::default(),
        hypothesis: acn_harness::run::HypothesisArg::None { seed: 7 },
        runs_dir: dir.path().join("runs"),
        start_dir: dir.path().to_path_buf(),
        engine_hash: acn_trace::identity::Digest::of(b"engine"),
        build: build(),
        profiles: Some(three()),
    };
    RunFixture { cfg, dir }
}

/// The bundle's trace.
pub fn read(dir: &std::path::Path) -> Trace {
    acn_trace::parquet_io::read_trace(dir, &acn_trace::schema::inventory().unwrap()).unwrap()
}

/// Serve `router` on a loopback port from a thread of its own; the base URL.
pub fn serve(router: axum::Router) -> String {
    let (tx, rx) = std::sync::mpsc::channel();
    std::thread::spawn(move || {
        let rt = tokio::runtime::Builder::new_current_thread()
            .enable_all()
            .build()
            .unwrap();
        rt.block_on(async move {
            let listener = tokio::net::TcpListener::bind("127.0.0.1:0").await.unwrap();
            tx.send(listener.local_addr().unwrap()).unwrap();
            axum::serve(listener, router).await.unwrap();
        });
    });
    format!("http://{}", rx.recv().unwrap())
}

/// The mock over HTTP on a virtual clock, so its waits are instant.
pub fn mock_server(profiles: Profiles) -> String {
    let mock = Mock::with_profiles(profiles, 3).unwrap();
    serve(acn_mockllm::server::router(
        mock,
        std::sync::Arc::new(acn_emu::clock::SimClock::new()),
    ))
}

/// An endpoint that is not the mock: an OpenAI-shaped `/v1/models`, no marker.
pub fn plain_server() -> String {
    use axum::routing::get;
    serve(axum::Router::new().route(
        "/v1/models",
        get(|| async { r#"{"object":"list","data":[]}"# }),
    ))
}
