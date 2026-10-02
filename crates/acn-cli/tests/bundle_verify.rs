//! `acn bundle verify` (TRC-23) under the CLI contract (CON-8).

#![allow(clippy::unwrap_used, clippy::expect_used, clippy::panic)] // CON-19: tests are exempt

use std::collections::BTreeMap;
use std::path::Path;
use std::process::Command;

use acn_trace::bundle::{Bundle, HypothesisRef, RunSpec};
use acn_trace::env::{self, RunHypothesis};
use acn_trace::fixture::{self, FixtureRun};
use acn_trace::identity::{BuildParts, Digest, HypStatus, Mode, RunParams};

fn acn(args: &[&str]) -> (Option<i32>, serde_json::Value) {
    let out = Command::new(env!("CARGO_BIN_EXE_acn"))
        .args(args)
        .output()
        .unwrap();
    let json = serde_json::from_slice(&out.stdout).unwrap();
    (out.status.code(), json)
}

fn bundle(runs: &Path) -> (std::path::PathBuf, Digest, Digest) {
    let build = BuildParts {
        cargo_lock: Digest::of(b"l"),
        rust_toolchain: Digest::of(b"t"),
        cargo_config: Digest::of(b"c"),
        source_hash: Digest::of(b"s"),
        target: "t",
        profile: "debug",
        features: "",
        rustflags: "",
    }
    .info()
    .unwrap();
    let outside = tempfile::tempdir().unwrap();
    let pf = env::preflight(outside.path(), Digest::of(b"e"), RunHypothesis::None).unwrap();
    let b = Bundle::create(
        runs,
        &pf,
        &build,
        RunSpec {
            seed: 3,
            mode: Mode::Sim,
            scenario_hash: Digest::of(b"scenario"),
            workload_hash: Digest::of(b"workload"),
            hypothesis: HypothesisRef::none(),
            params: RunParams {
                backend: "mockllm".into(),
                model: "m".into(),
                hyp_status: HypStatus::Candidate,
                arms: vec!["control".into()],
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
        seed: 3,
        replicate: 0,
        engine_hash: Digest::of(b"e"),
        build_hash: Digest::from_hex(&build.build_hash).unwrap(),
    })
    .unwrap();
    let w = b.finish(&t).unwrap();
    (w.dir, w.run_id, w.bundle_digest)
}

/// Cites: TRC-23, CON-8
#[test]
fn verify_prints_the_run_id_and_bundle_digest() {
    let runs = tempfile::tempdir().unwrap();
    let (dir, run_id, digest) = bundle(runs.path());
    let (code, json) = acn(&["bundle", "verify", dir.to_str().unwrap()]);
    assert_eq!(code, Some(0), "{json}");
    assert_eq!(json["ok"], true);
    assert_eq!(json["run_id"], run_id.to_hex().as_str());
    assert_eq!(json["bundle_digest"], digest.to_hex().as_str());
}

/// Cites: TRC-23, CON-8
#[test]
fn a_tampered_bundle_is_a_json_error_with_exit_one() {
    let runs = tempfile::tempdir().unwrap();
    let (dir, _, _) = bundle(runs.path());
    std::fs::write(dir.join("unlisted.bin"), b"x").unwrap();
    let (code, json) = acn(&["bundle", "verify", dir.to_str().unwrap()]);
    assert_eq!(code, Some(1));
    assert_eq!(json["ok"], false);
    assert!(json["error"].as_str().unwrap().contains("unlisted.bin"));
    let (code, json) = acn(&["bundle", "verify", "/nonexistent/bundle"]);
    assert_eq!((code, &json["ok"]), (Some(1), &serde_json::json!(false)));
}

/// Cites: TRC-35, CON-8
#[test]
fn verify_views_recomputes_and_reports_it() {
    let runs = tempfile::tempdir().unwrap();
    let (dir, run_id, _) = bundle(runs.path());
    let (code, json) = acn(&["bundle", "verify", "--views", dir.to_str().unwrap()]);
    assert_eq!(code, Some(0), "{json}");
    assert_eq!(json["views_recomputed"], true);
    assert_eq!(json["run_id"], run_id.to_hex().as_str());
}
