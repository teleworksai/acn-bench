//! SPEC 010 acceptance (TRC-28): a bundle exported to OTLP JSON and re-imported
//! yields identical views, and identical tables.

#![allow(clippy::unwrap_used, clippy::expect_used, clippy::panic)] // CON-19: tests are exempt

use std::collections::BTreeMap;

use acn_trace::bundle::{self, Bundle, HypothesisRef, RunSpec};
use acn_trace::env::{self, RunHypothesis};
use acn_trace::fixture::{self, FixtureRun};
use acn_trace::identity::{BuildParts, Digest, HypStatus, Mode, RunParams};
use acn_trace::{ingest, otlp, parquet_io, schema};

/// Cites: TRC-28, TRC-35
#[test]
fn export_to_otlp_json_and_reimport_yields_identical_views() {
    let runs = tempfile::tempdir().unwrap();
    let build = BuildParts {
        cargo_lock: Digest::of(b"l"),
        rust_toolchain: Digest::of(b"t"),
        cargo_config: Digest::of(b"c"),
        source_hash: Digest::of(b"s"),
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
        runs.path(),
        &pf,
        &build,
        RunSpec {
            seed: 11,
            mode: Mode::Sim,
            scenario_hash: Digest::of(b"scenario"),
            workload_hash: Digest::of(b"workload"),
            hypothesis: HypothesisRef::none(),
            params: RunParams {
                backend: "mockllm".into(),
                model: "fixture".into(),
                hyp_status: HypStatus::Candidate,
                arms: vec!["treatment".into()],
                replicates: 1,
                vary: BTreeMap::new(),
                opts: BTreeMap::new(),
            },
            endpoint_host: None,
            execution_order: None,
            started_at: None,
        },
    )
    .unwrap();
    let t = fixture::session(&FixtureRun {
        run_id: b.run_id().into(),
        seed: 11,
        replicate: 0,
        engine_hash: engine,
        build_hash: Digest::from_hex(&build.build_hash).unwrap(),
    })
    .unwrap();
    let w = b.finish(&t).unwrap();

    let inv = schema::inventory().unwrap();
    let from_bundle = parquet_io::read_trace(&w.dir, &inv).unwrap();
    let text = serde_json::to_string(&otlp::to_json(&from_bundle).unwrap()).unwrap();
    let imported = otlp::from_json(&serde_json::from_str(&text).unwrap()).unwrap();
    assert_eq!(imported, from_bundle, "the tables survive the round trip");
    for (view, batch) in ingest::views(&inv, &schema::views().unwrap(), &imported).unwrap() {
        let on_disk = std::fs::read(w.dir.join(&view.file)).unwrap();
        assert_eq!(
            parquet_io::encode(&batch).unwrap(),
            on_disk,
            "{} is identical after export and import",
            view.file
        );
    }
    // Exported as several requests, as to a collector, it is still the bundle.
    let chunks = otlp::to_json_chunks(&from_bundle, 2).unwrap();
    assert!(chunks.len() > 1);
    assert_eq!(otlp::from_json_many(&chunks).unwrap(), from_bundle);
    assert!(bundle::verify_views(&w.dir).is_ok());
}
