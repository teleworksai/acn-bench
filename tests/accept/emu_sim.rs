//! SPEC 020 §4 acceptance (T11.3): a `sim` run whose calls cross a scenario's
//! network. Twice byte-identical; link spans under each `chat`, from the
//! `acn-emu` resource, linked to the one `acn.scenario` span; drops recorded as
//! a timeout and as cut, retried streams. (That a zero-delay path batches as no
//! scenario does is `crates/acn-harness/tests/network.rs`: two runs cannot show
//! it, since the scenario moves the run id and so every prompt's marker.)

#![allow(clippy::unwrap_used, clippy::expect_used, clippy::panic)] // CON-19: tests are exempt

use std::collections::BTreeMap;
use std::path::{Path, PathBuf};

use acn_harness::agent::Opts;
use acn_harness::run::{HypothesisArg, RunConfig, run, run_with_scenario};
use acn_harness::wire::Backend;
use acn_trace::bundle;
use acn_trace::identity::{BuildParts, Digest, Mode};
use acn_trace::model::{AttrValue, SpanRow, Trace};

fn repo(rel: &str) -> PathBuf {
    PathBuf::from(concat!(env!("CARGO_MANIFEST_DIR"), "/../..")).join(rel)
}

fn fixture(name: &str) -> PathBuf {
    PathBuf::from(concat!(env!("CARGO_MANIFEST_DIR"), "/fixtures/scenarios")).join(name)
}

fn cfg(runs: &Path, workload: &str, model: &str, vary: &[(&str, &str)]) -> RunConfig {
    RunConfig {
        workload: repo(workload),
        backend: Backend::Mockllm,
        model: model.into(),
        mode: Mode::Sim,
        arm: "treatment".into(),
        replicates: 2,
        vary: vary
            .iter()
            .map(|(k, v)| ((*k).to_owned(), (*v).to_owned()))
            .collect::<BTreeMap<_, _>>(),
        opts: Opts::default(),
        hypothesis: HypothesisArg::None { seed: 20_261_006 },
        runs_dir: runs.to_path_buf(),
        start_dir: runs.to_path_buf(),
        engine_hash: Digest::of(b"engine"),
        build: BuildParts {
            cargo_lock: Digest::of(b"lock"),
            rust_toolchain: Digest::of(b"toolchain"),
            cargo_config: Digest::of(b"config"),
            source_hash: Digest::of(b"source"),
            target: "accept",
            profile: "debug",
            features: "",
            rustflags: "",
        }
        .info()
        .unwrap(),
        profiles: None,
    }
}

fn files(dir: &Path) -> BTreeMap<String, Vec<u8>> {
    let mut out = BTreeMap::new();
    let mut stack = vec![dir.to_path_buf()];
    while let Some(d) = stack.pop() {
        for e in std::fs::read_dir(&d).unwrap() {
            let p = e.unwrap().path();
            let rel = p
                .strip_prefix(dir)
                .unwrap()
                .to_string_lossy()
                .replace('\\', "/");
            if p.is_dir() {
                if rel != bundle::LOGS {
                    stack.push(p);
                }
            } else {
                out.insert(rel, std::fs::read(&p).unwrap());
            }
        }
    }
    out
}

fn trace(dir: &Path) -> Trace {
    acn_trace::parquet_io::read_trace(dir, &acn_trace::schema::inventory().unwrap()).unwrap()
}

fn s(span: &SpanRow, key: &str) -> Option<String> {
    match span.attrs.get(key)? {
        AttrValue::String(v) => Some(v.clone()),
        _ => None,
    }
}

fn b(span: &SpanRow, key: &str) -> Option<bool> {
    match span.attrs.get(key)? {
        AttrValue::Bool(v) => Some(*v),
        _ => None,
    }
}

fn i(span: &SpanRow, key: &str) -> Option<i64> {
    match span.attrs.get(key)? {
        AttrValue::Int(v) => Some(*v),
        _ => None,
    }
}

fn named<'a>(t: &'a Trace, name: &str) -> Vec<&'a SpanRow> {
    t.spans.iter().filter(|s| s.name == name).collect()
}

/// Cites: EMU-38, EMU-36, EMU-37, TRC-24, CON-5
#[test]
fn a_scenario_run_twice_is_byte_identical() {
    let sc = repo("scenarios/synthetic/cellular-handover.toml");
    let (a, b) = (tempfile::tempdir().unwrap(), tempfile::tempdir().unwrap());
    let c = |d: &Path| cfg(d, "workloads/harness-smoke.toml", "mock-explicit", &[]);
    let wa = run_with_scenario(&c(a.path()), Some(&sc)).unwrap();
    let wb = run_with_scenario(&c(b.path()), Some(&sc)).unwrap();
    assert_eq!(wa.run_id, wb.run_id);
    assert_eq!(files(&wa.dir), files(&wb.dir));
    bundle::verify_views(&wa.dir).unwrap();
    // The scenario is part of the run's identity (CON-29).
    let n = tempfile::tempdir().unwrap();
    let wn = run(&c(n.path())).unwrap();
    assert_ne!(wa.run_id, wn.run_id);
}

/// Cites: EMU-36, EMU-37, EMU-32
#[test]
fn link_spans_sit_under_their_calls_and_link_to_the_scenario_span() {
    let path = repo("scenarios/synthetic/cellular-handover.toml");
    let d = tempfile::tempdir().unwrap();
    let w = run_with_scenario(
        &cfg(
            d.path(),
            "workloads/harness-smoke.toml",
            "mock-explicit",
            &[],
        ),
        Some(&path),
    )
    .unwrap();
    let t = trace(&w.dir);
    let scenario = named(&t, "acn.scenario");
    assert_eq!(scenario.len(), 1, "one scenario span per run");
    let scenario = scenario[0];
    let hash = blake3::hash(&std::fs::read(&path).unwrap())
        .to_hex()
        .to_string();
    assert_eq!(
        s(scenario, "acn.scenario.hash").as_deref(),
        Some(hash.as_str())
    );
    assert_eq!(
        s(scenario, "acn.scenario.toml").unwrap(),
        std::fs::read_to_string(&path).unwrap()
    );
    assert_eq!(scenario.start_ns, 0);
    // Sessions record the scenario's hash.
    for session in named(&t, "acn.session") {
        assert_eq!(
            s(session, "acn.scenario.hash").as_deref(),
            Some(hash.as_str())
        );
    }
    // Every link span: under a chat span, from the acn-emu resource, linked to
    // the scenario span, with TRC-15's attributes.
    let links = named(&t, "acn.link");
    assert!(!links.is_empty());
    let chats: BTreeMap<[u8; 8], &SpanRow> = named(&t, "chat")
        .into_iter()
        .map(|c| (c.span_id, c))
        .collect();
    let emu: Vec<i32> = t
        .resources
        .iter()
        .filter(|r| r.attrs.get("service.name") == Some(&AttrValue::String("acn-emu".into())))
        .map(|r| r.resource_id)
        .collect();
    assert_eq!(emu.len(), 1, "one acn-emu resource");
    let mut ups = 0;
    for l in &links {
        let parent = chats
            .get(&l.parent_span_id.unwrap())
            .expect("a link's parent is a chat");
        assert!(l.start_ns >= parent.start_ns);
        assert_eq!(l.resource_id, emu[0]);
        assert!(t.links.iter().any(|k| k.span_id == l.span_id
            && k.linked_span_id == scenario.span_id
            && k.linked_trace_id == scenario.trace_id));
        assert_eq!(s(l, "acn.link.id").as_deref(), Some("radio"));
        let dir = s(l, "acn.link.direction").unwrap();
        let model = s(l, "acn.link.model").unwrap();
        if dir == "up" {
            ups += 1;
            assert_eq!(model, "outage+gilbert_elliott+rate+delay+reorder");
        } else {
            assert_eq!(model, "outage+iid+rate+delay");
        }
        assert!(i(l, "acn.link.dequeue_ns").unwrap() >= i(l, "acn.link.enqueue_ns").unwrap());
        assert!(i(l, "acn.link.bytes").unwrap() > 0);
        assert!(b(l, "acn.link.dropped").is_some() && b(l, "acn.link.reordered").is_some());
    }
    // One request per attempt, at least one per call.
    assert!(ups >= chats.len());
}

/// Cites: EMU-35, EMU-37
#[test]
fn a_lost_request_ends_its_call_at_the_deadline() {
    let d = tempfile::tempdir().unwrap();
    let mut c = cfg(
        d.path(),
        "workloads/harness-smoke.toml",
        "mock-explicit",
        &[],
    );
    c.replicates = 1;
    c.opts.request_timeout_ms = 2_000;
    let w = run_with_scenario(&c, Some(&fixture("uplink-down.toml"))).unwrap();
    let t = trace(&w.dir);
    let first = named(&t, "chat")
        .into_iter()
        .min_by_key(|c| c.start_ns)
        .unwrap();
    assert_eq!(s(first, "acn.call.error_class").as_deref(), Some("timeout"));
    assert_eq!(first.end_ns - first.start_ns, 2_000_000_000);
    // Its request's link span is dropped, its time zero.
    let up = named(&t, "acn.link")
        .into_iter()
        .find(|l| l.parent_span_id == Some(first.span_id))
        .unwrap();
    assert_eq!(b(up, "acn.link.dropped"), Some(true));
    assert_eq!(i(up, "acn.link.dequeue_ns"), i(up, "acn.link.enqueue_ns"));
    // The scenario span records the window that dropped it.
    let outages: Vec<_> = t
        .events
        .iter()
        .filter(|e| e.name == "acn.scenario.outage")
        .collect();
    assert_eq!(outages.len(), 1);
    assert_eq!(
        outages[0].attrs.get("cause"),
        Some(&AttrValue::String("scheduled".into()))
    );
}

/// Cites: EMU-35, EMU-34, HAR-24
#[test]
fn lost_stream_events_cut_the_stream_and_the_call_retries() {
    let d = tempfile::tempdir().unwrap();
    let c = cfg(
        d.path(),
        "workloads/harness-smoke.toml",
        "mock-explicit",
        &[],
    );
    let w = run_with_scenario(&c, Some(&fixture("lossy-down.toml"))).unwrap();
    let t = trace(&w.dir);
    let links = named(&t, "acn.link");
    assert!(links.iter().any(|l| b(l, "acn.link.dropped") == Some(true)
        && s(l, "acn.link.direction").as_deref() == Some("down")));
    // Some call needed a retry after a cut stream.
    assert!(
        named(&t, "chat")
            .iter()
            .any(|c| i(c, "acn.call.retries").unwrap_or(0) > 0)
    );
    // Receive times, not send times: every call's first token arrives at least
    // the uplink and downlink delays (5 ms each) after its start.
    for chat in named(&t, "chat") {
        if let Some(AttrValue::Float(ttft)) = chat.attrs.get("acn.call.ttft_ms") {
            assert!(*ttft >= 10.0, "{ttft}");
        }
    }
}

/// Cites: EMU-32, EMU-39
#[test]
fn a_scenario_needs_one_path_and_sim() {
    let d = tempfile::tempdir().unwrap();
    let c = cfg(
        d.path(),
        "workloads/harness-smoke.toml",
        "mock-explicit",
        &[],
    );
    let e = run_with_scenario(&c, Some(&fixture("two-paths.toml"))).unwrap_err();
    assert!(e.to_string().contains("exactly one path"), "{e}");
    let mut live = c.clone();
    live.mode = Mode::Live;
    let e = run_with_scenario(&live, Some(&fixture("uplink-down.toml"))).unwrap_err();
    assert!(e.to_string().contains("sim only"), "{e}");
    assert!(
        std::fs::read_dir(d.path()).unwrap().next().is_none(),
        "nothing written"
    );
}
