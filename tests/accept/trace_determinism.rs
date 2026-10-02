//! SPEC 010 acceptance: two `sim` runs with the same inputs produce byte-identical
//! bundles, every file but `logs/` (TRC-24), with ids from the seeded generator
//! (TRC-27). The fixture scenario is the `acn-trace` fixture session, run once per
//! replicate through the OpenTelemetry SDK; `acn-emu` replaces it when the sim
//! engine lands (T11). The five views are part of every bundle and of the comparison.

#![allow(clippy::unwrap_used, clippy::expect_used, clippy::panic)] // CON-19: tests are exempt

use std::collections::BTreeMap;
use std::path::Path;

use acn_trace::bundle::{self, Bundle, HypothesisRef, RunSpec};
use acn_trace::env::{self, RunHypothesis};
use acn_trace::fixture::{self, FixtureRun};
use acn_trace::identity::{BuildParts, Digest, HypStatus, Mode, RunParams, Value};
use acn_trace::model::Trace;

const SEED: u64 = 20_261_002;
const REPLICATES: u32 = 3;

/// One sim run of the fixture scenario into `runs`.
fn run(runs: &Path, logs: &str, seed: u64) -> bundle::Written {
    let build = BuildParts {
        cargo_lock: Digest::of(b"lock"),
        rust_toolchain: Digest::of(b"toolchain"),
        cargo_config: Digest::of(b"config"),
        source_hash: Digest::of(b"source"),
        target: "fixture",
        profile: "debug",
        features: "",
        rustflags: "",
    }
    .info()
    .unwrap();
    let outside = tempfile::tempdir().unwrap();
    let engine = Digest::of(b"engine");
    let pf = env::preflight(outside.path(), engine, RunHypothesis::None).unwrap();
    let b = Bundle::create(
        runs,
        &pf,
        &build,
        RunSpec {
            seed,
            mode: Mode::Sim,
            scenario_hash: Digest::of(b"fixture scenario"),
            workload_hash: Digest::of(b"fixture workload"),
            hypothesis: HypothesisRef::none(),
            params: RunParams {
                backend: "mockllm".into(),
                model: "fixture".into(),
                hyp_status: HypStatus::Candidate,
                arms: vec!["treatment".into()],
                replicates: REPLICATES,
                vary: BTreeMap::from([("ratio".into(), Value::Float(0.25))]),
                opts: BTreeMap::new(),
            },
            endpoint_host: None,
            execution_order: None,
            started_at: None,
        },
    )
    .unwrap();
    let mut trace = Trace::default();
    for i in 0..REPLICATES {
        let t = fixture::session(&FixtureRun {
            run_id: b.run_id().into(),
            seed,
            replicate: i,
            engine_hash: engine,
            build_hash: Digest::from_hex(&build.build_hash).unwrap(),
        })
        .unwrap();
        trace.spans.extend(t.spans);
        trace.events.extend(t.events);
        trace.links.extend(t.links);
        if trace.resources.is_empty() {
            trace.resources = t.resources;
        }
    }
    trace.sort();
    std::fs::write(b.logs_dir().join("stderr.log"), logs).unwrap();
    b.finish(&trace).unwrap()
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

/// Cites: TRC-24, TRC-27, CON-5
#[test]
fn two_sim_runs_with_the_same_inputs_are_byte_identical() {
    let (a, b) = (tempfile::tempdir().unwrap(), tempfile::tempdir().unwrap());
    // Different log contents: logs/ is the one place allowed to differ.
    let wa = run(a.path(), "first run's stderr", SEED);
    let wb = run(b.path(), "second run's stderr, longer", SEED);
    assert_eq!(wa.run_id, wb.run_id);
    assert_eq!(wa.bundle_digest, wb.bundle_digest);
    eprintln!("bundle_digest {}", wa.bundle_digest);
    let (fa, fb) = (files(&wa.dir), files(&wb.dir));
    assert_eq!(fa.keys().collect::<Vec<_>>(), fb.keys().collect::<Vec<_>>());
    for (name, bytes) in &fa {
        assert!(bytes == &fb[name], "{name} differs between two sim runs");
    }
    assert!(fa.contains_key("spans.parquet") && fa.contains_key(bundle::MANIFEST));
    bundle::verify(&wa.dir).unwrap();
    bundle::verify(&wb.dir).unwrap();
}

/// Cites: TRC-24
#[test]
fn a_different_seed_changes_the_identity_and_the_bytes() {
    let (a, b) = (tempfile::tempdir().unwrap(), tempfile::tempdir().unwrap());
    let wa = run(a.path(), "", SEED);
    let wb = run(b.path(), "", SEED + 1);
    assert_ne!(wa.run_id, wb.run_id);
    assert_ne!(wa.bundle_digest, wb.bundle_digest);
    assert_ne!(
        std::fs::read(wa.dir.join("spans.parquet")).unwrap(),
        std::fs::read(wb.dir.join("spans.parquet")).unwrap(),
        "the seeded ids differ"
    );
}
