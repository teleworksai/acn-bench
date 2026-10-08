//! `acn run --from-run-id` (SPEC 140 P16-2, P16-6): one JSON object, and exit
//! 1 iff `ok` is false (CON-8).

#![allow(clippy::unwrap_used, clippy::expect_used, clippy::panic)] // CON-19: tests are exempt

use std::path::Path;
use std::process::Command;

use serde_json::Value;

fn acn(dir: &Path, args: &[&str]) -> (Option<i32>, Value) {
    let out = Command::new(env!("CARGO_BIN_EXE_acn"))
        .current_dir(dir)
        .args(args)
        .output()
        .expect("spawn acn");
    let stdout = String::from_utf8(out.stdout).expect("utf8");
    let json: Value = serde_json::from_str(stdout.trim())
        .unwrap_or_else(|e| panic!("stdout is not one JSON object: {e}\n{stdout}"));
    (out.status.code(), json)
}

/// Cites: P16-2, P16-6, CON-8
#[test]
fn a_run_regenerates_from_its_id_and_an_unknown_id_exits_one() {
    let dir = tempfile::tempdir().unwrap();
    let d = dir.path();
    std::fs::create_dir_all(d.join("workloads")).unwrap();
    std::fs::copy(
        concat!(
            env!("CARGO_MANIFEST_DIR"),
            "/../../workloads/harness-smoke.toml"
        ),
        d.join("workloads/w.toml"),
    )
    .unwrap();
    let (code, j) = acn(
        d,
        &[
            "harness",
            "run",
            "--workload",
            "workloads/w.toml",
            "--backend",
            "mockllm",
            "--model",
            "mock-auto",
            "--seed",
            "3",
        ],
    );
    assert_eq!(code, Some(0), "{j}");
    let id = j["run_id"].as_str().unwrap();
    let (code, v) = acn(d, &["run", "--from-run-id", id]);
    assert_eq!(code, Some(0), "{v}");
    assert_eq!(v["run_id"], id);
    assert_eq!(v["differ"], serde_json::json!([]));
    assert!(
        d.join(v["dir"].as_str().unwrap())
            .join("manifest.json")
            .exists(),
        "{v}"
    );
    assert_eq!(
        (&v["ok"], &v["identical"], &v["same_build"]),
        (&true.into(), &true.into(), &true.into()),
        "{v}"
    );
    let (code, v) = acn(d, &["run", "--from-run-id", &"0".repeat(64)]);
    assert_eq!((code, &v["ok"]), (Some(1), &false.into()), "{v}");
    assert_eq!(v["code"], "unknown_run");
}
