//! HAR-50: `acn harness run` executes one cell and one arm into one bundle and
//! prints one JSON object (CON-8).

#![allow(clippy::unwrap_used, clippy::expect_used, clippy::panic)] // CON-19: tests are exempt

use std::process::Command;

const SMOKE: &str = concat!(
    env!("CARGO_MANIFEST_DIR"),
    "/../../workloads/harness-smoke.toml"
);

fn acn(args: &[&str]) -> (Option<i32>, serde_json::Value) {
    let out = Command::new(env!("CARGO_BIN_EXE_acn"))
        .args(args)
        .output()
        .expect("spawn acn");
    let stdout = String::from_utf8(out.stdout).expect("utf8");
    (
        out.status.code(),
        serde_json::from_str(stdout.trim()).unwrap_or_else(|e| panic!("{e}: {stdout}")),
    )
}

/// Cites: HAR-50, CON-8
#[test]
fn harness_run_writes_one_bundle_and_prints_its_identity() {
    let runs = tempfile::tempdir().unwrap();
    let runs_dir = runs.path().to_str().unwrap();
    let args = [
        "harness",
        "run",
        "--workload",
        SMOKE,
        "--backend",
        "mockllm",
        "--model",
        "mock-auto",
        "--seed",
        "3",
        "--replicates",
        "2",
        "--vary",
        "tool_order_stable=false",
        "--runs-dir",
        runs_dir,
    ];
    let (code, json) = acn(&args);
    assert_eq!(code, Some(0), "{json}");
    assert_eq!(json["ok"], true);
    let run_id = json["run_id"].as_str().unwrap();
    assert_eq!(run_id.len(), 64);
    assert_eq!(json["bundle_digest"].as_str().unwrap().len(), 64);
    let dir = runs.path().join(run_id);
    let (_, verified) = acn(&["bundle", "verify", "--views", dir.to_str().unwrap()]);
    assert_eq!(verified["run_id"], run_id);
    assert_eq!(verified["bundle_digest"], json["bundle_digest"]);
    // The same command again: the bundle is never replaced (CON-29).
    let (code, again) = acn(&args);
    assert_eq!(code, Some(1));
    assert!(again["error"].as_str().unwrap().contains("never replaced"));
    // A misspelt knob is refused before anything runs.
    let (code, bad) = acn(&[
        "harness",
        "run",
        "--workload",
        SMOKE,
        "--backend",
        "mockllm",
        "--model",
        "mock-auto",
        "--seed",
        "3",
        "--vary",
        "tool_order=false",
        "--runs-dir",
        runs_dir,
    ]);
    assert_eq!(code, Some(1));
    assert!(bad["error"].as_str().unwrap().contains("not a knob"));
}
