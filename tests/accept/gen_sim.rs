//! SPEC 050 acceptance (GEN-20 to GEN-23): a generator run on the harness's
//! run path is bit-identical in `sim`, crosses a scenario's network, and its
//! `live` twin on the served mock draws the same plans.

#![allow(clippy::unwrap_used, clippy::expect_used, clippy::panic)] // CON-19: tests are exempt

use std::collections::BTreeMap;
use std::path::Path;

use acn_gen::run::{GenConfig, run};
use acn_harness::agent::Opts;
use acn_harness::run::HypothesisArg;
use acn_trace::identity::{BuildParts, Digest, Mode};
use acn_trace::model::{AttrValue, SpanRow, Trace};

const SHEET: &str = r#"schema_version = 1
placeholder = true
doc = "the generator's acceptance sheet"
model = "mock-agentic"
sessions = 3
system_tokens = 200
summary_instruction_tokens = 8
summary_max_tokens = 32
compact_at_tokens = 0
session_start_ns = { uniform = [0, 200_000_000] }
turns_per_session = { uniform = [2, 3] }
think_time_ns = { uniform = [10_000_000, 50_000_000] }
chain_length = { uniform = [1, 3] }
fanout_width = { weighted = [[0, 2], [2, 1]] }
user_tokens = { uniform = [10, 30] }
answer_tokens = { uniform = [8, 24] }
tool_class = { weighted = [["file", 2], ["search", 1]] }

[tool_result_tokens]
file = { uniform = [20, 80] }
search = { uniform = [10, 40] }

[tool_duration_ns]
file = { uniform = [1_000_000, 3_000_000] }
search = { uniform = [2_000_000, 6_000_000] }
"#;

fn config(dir: &Path, mode: Mode) -> GenConfig {
    let sheet = dir.join("sheet.toml");
    std::fs::write(&sheet, SHEET).unwrap();
    GenConfig {
        sheet,
        mode,
        arm: "treatment".into(),
        replicates: 2,
        vary: BTreeMap::new(),
        // A wedged loopback server fails fast instead of hanging the suite.
        opts: if mode == Mode::Live {
            Opts {
                request_timeout_ms: 10_000,
                max_retries: 0,
                ..Opts::default()
            }
        } else {
            Opts::default()
        },
        hypothesis: HypothesisArg::None { seed: 20_261_006 },
        runs_dir: dir.join("runs"),
        start_dir: dir.to_path_buf(),
        engine_hash: Digest::of(b"engine"),
        build: BuildParts {
            cargo_lock: Digest::of(b"lock"),
            rust_toolchain: Digest::of(b"toolchain"),
            cargo_config: Digest::of(b"config"),
            source_hash: Digest::of(b"build"),
            target: "test",
            profile: "debug",
            features: "",
            rustflags: "",
        }
        .info()
        .unwrap(),
        // Mock timing a thousand times shorter: a `live` run waits on the
        // wall clock. Profiles are not in the run_id (MLM-51).
        profiles: Some(
            acn_mockllm::profile::Profiles::parse(
                &acn_mockllm::profile::PROFILES_TOML
                    .replace("itl_ns = 20_000_000", "itl_ns = 20_000")
                    .replace("itl_jitter_ns = 2_000_000", "itl_jitter_ns = 2_000")
                    .replace("prefill_base_ns = 20_000_000", "prefill_base_ns = 20_000"),
            )
            .unwrap(),
        ),
    }
}

fn read(dir: &Path) -> Trace {
    acn_trace::parquet_io::read_trace(dir, &acn_trace::schema::inventory().unwrap()).unwrap()
}

fn text<'a>(s: &'a SpanRow, key: &str) -> Option<&'a str> {
    match s.attrs.get(key) {
        Some(AttrValue::String(v)) => Some(v),
        _ => None,
    }
}

fn children<'a>(t: &'a Trace, parent: &SpanRow, name: &str) -> Vec<&'a SpanRow> {
    let mut v: Vec<&SpanRow> = t
        .spans
        .iter()
        .filter(|s| s.name == name && s.parent_span_id == Some(parent.span_id))
        .collect();
    v.sort_by_key(|s| (s.start_ns, s.span_id));
    v
}

/// Every turn's plan as the bundle records it: its main chain's tool
/// classes and its sub-agents' chain lengths (sorted: they start at one
/// instant). Across sessions and replicates, as a sorted multiset.
fn plans(t: &Trace) -> Vec<(Vec<String>, Vec<usize>)> {
    let mut out = Vec::new();
    for turn in t.spans.iter().filter(|s| s.name == "acn.turn") {
        let tools = children(t, turn, "execute_tool");
        let classes = tools
            .iter()
            .map(|x| text(x, "acn.tool.class").unwrap().to_owned())
            .collect();
        let mut subs: Vec<usize> = tools
            .iter()
            .flat_map(|x| children(t, x, "invoke_agent"))
            .map(|a| children(t, a, "chat").len())
            .collect();
        subs.sort_unstable();
        out.push((classes, subs));
    }
    out.sort();
    out
}

/// Cites: GEN-23, GEN-20, CON-5
#[test]
fn a_sim_run_twice_is_bit_identical() {
    let (a, b) = (tempfile::tempdir().unwrap(), tempfile::tempdir().unwrap());
    let x = run(&config(a.path(), Mode::Sim), None).unwrap();
    let y = run(&config(b.path(), Mode::Sim), None).unwrap();
    assert_eq!(x.written.run_id, y.written.run_id);
    assert_eq!(x.written.bundle_digest, y.written.bundle_digest);
    assert_eq!((x.sessions, x.calls), (y.sessions, y.calls));
    assert_eq!(x.sessions, 6);
    // The bundle verifies with its views recomputed (TRC-23, TRC-35).
    acn_trace::bundle::verify_views(&x.written.dir).unwrap();
    // The counts are the bundle's (GEN-22): over every replicate.
    let t = read(&x.written.dir);
    let count = |name: &str| t.spans.iter().filter(|s| s.name == name).count() as u64;
    assert_eq!(x.sessions, count("acn.session"));
    assert_eq!(x.calls, count("chat"));
}

/// Cites: GEN-20
#[test]
fn a_scenarios_network_carries_every_call() {
    let dir = tempfile::tempdir().unwrap();
    let sc = dir.path().join("p.toml");
    std::fs::write(
        &sc,
        "schema_version = 1\nname = \"p\"\n\n[[link]]\nname = \"p\"\ndirection = \"up\"\n\n[link.delay]\ndelay_us = 20000\njitter_us = 0\n\n[[link]]\nname = \"p\"\ndirection = \"down\"\n",
    )
    .unwrap();
    let plain = run(&config(dir.path(), Mode::Sim), None).unwrap();
    let other = tempfile::tempdir().unwrap();
    let carried = run(&config(other.path(), Mode::Sim), Some(&sc)).unwrap();
    let (t0, t1) = (read(&plain.written.dir), read(&carried.written.dir));
    assert!(t1.spans.iter().any(|s| s.name == "acn.link"));
    assert!(!t0.spans.iter().any(|s| s.name == "acn.link"));
    // The same plans, now 20 ms later per request.
    assert_eq!(plans(&t0), plans(&t1));
    assert_ne!(plain.written.run_id, carried.written.run_id);
}

/// Cites: GEN-20, GEN-22, GEN-4, HAR-26
#[test]
fn a_live_run_on_the_served_mock_draws_the_plans_of_its_sim_twin() {
    let (a, b) = (tempfile::tempdir().unwrap(), tempfile::tempdir().unwrap());
    let sim = run(&config(a.path(), Mode::Sim), None).unwrap();
    let live = run(&config(b.path(), Mode::Live), None).unwrap();
    let (ts, tl) = (read(&sim.written.dir), read(&live.written.dir));
    // Tool classes, chain lengths and widths are the plans, drawn before any
    // call, so they do not depend on the mode (GEN-4). The mock's outputs and
    // the timing do, so they are not compared.
    assert_eq!(plans(&ts), plans(&tl));
    assert_eq!(sim.sessions, live.sessions);
    assert_eq!(sim.calls, live.calls);
    // No endpoint given: the harness served the mock (GEN-22, HAR-26).
    let manifest: serde_json::Value =
        serde_json::from_slice(&std::fs::read(live.written.dir.join("manifest.json")).unwrap())
            .unwrap();
    assert_eq!(manifest["endpoint_host"], "loopback");
    assert_eq!(manifest["mode"], "live");
    acn_trace::bundle::verify_views(&live.written.dir).unwrap();
}
