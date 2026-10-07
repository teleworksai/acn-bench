//! `acn gen run` (SPEC 050 GEN-22): one JSON object per run, and a knob
//! `vary` refused (GEN-21).

#![allow(clippy::unwrap_used, clippy::expect_used, clippy::panic)] // CON-19: tests are exempt

use std::process::Command;

const SHEET: &str = r#"schema_version = 1
placeholder = true
doc = "a small sheet for the CLI test"
model = "mock-agentic"
sessions = 2
system_tokens = 50
summary_instruction_tokens = 8
summary_max_tokens = 32
compact_at_tokens = 0
session_start_ns = { uniform = [0, 1_000_000_000] }
turns_per_session = { const = 2 }
think_time_ns = { const = 500_000_000 }
chain_length = { uniform = [1, 2] }
fanout_width = { const = 0 }
user_tokens = { const = 10 }
answer_tokens = { const = 16 }
tool_class = { weighted = [["file", 1]] }

[tool_result_tokens]
file = { const = 20 }

[tool_duration_ns]
file = { const = 1_000_000 }
"#;

fn acn(dir: &std::path::Path, args: &[&str]) -> (Option<i32>, serde_json::Value) {
    let out = Command::new(env!("CARGO_BIN_EXE_acn"))
        .current_dir(dir)
        .args(args)
        .output()
        .expect("spawn acn");
    let stdout = String::from_utf8(out.stdout).expect("utf8");
    let json: serde_json::Value = serde_json::from_str(stdout.trim())
        .unwrap_or_else(|e| panic!("stdout is not one JSON object: {e}\n{stdout}"));
    (out.status.code(), json)
}

/// Cites: GEN-22, CON-8
#[test]
fn gen_run_prints_one_object_with_its_bundle_and_counts() {
    let dir = tempfile::tempdir().unwrap();
    std::fs::write(dir.path().join("sheet.toml"), SHEET).unwrap();
    let (code, j) = acn(
        dir.path(),
        &["gen", "run", "--sheet", "sheet.toml", "--seed", "3"],
    );
    assert_eq!(code, Some(0), "{j}");
    assert_eq!(j["ok"], true);
    for k in ["run_id", "bundle_digest", "dir"] {
        assert!(j[k].is_string(), "{k}: {j}");
    }
    assert_eq!(j["sessions"], 2);
    // The counts are the bundle's (GEN-22).
    let t = acn_trace::parquet_io::read_trace(
        &dir.path().join(j["dir"].as_str().unwrap()),
        &acn_trace::schema::inventory().unwrap(),
    )
    .unwrap();
    let count = |name: &str| t.spans.iter().filter(|s| s.name == name).count() as u64;
    assert_eq!(j["sessions"].as_u64(), Some(count("acn.session")));
    assert_eq!(j["calls"].as_u64(), Some(count("chat")));
    // The bundle verifies (TRC-23).
    let (code, v) = acn(
        dir.path(),
        &["bundle", "verify", j["dir"].as_str().unwrap()],
    );
    assert_eq!(code, Some(0), "{v}");
}

/// Cites: GEN-21, GEN-22
#[test]
fn gen_run_refuses_a_knob_vary_with_ok_false() {
    let dir = tempfile::tempdir().unwrap();
    std::fs::write(dir.path().join("sheet.toml"), SHEET).unwrap();
    let (code, j) = acn(
        dir.path(),
        &[
            "gen",
            "run",
            "--sheet",
            "sheet.toml",
            "--seed",
            "3",
            "--vary",
            "tool_order_stable=false",
        ],
    );
    assert_eq!(j["ok"], false, "{j}");
    assert_ne!(code, Some(0));
    assert!(j["error"].as_str().unwrap().contains("GEN-21"), "{j}");
    // Refused before the run has an identity: nothing was written.
    let runs = dir.path().join("runs");
    assert!(!runs.exists() || std::fs::read_dir(&runs).unwrap().next().is_none());
}

/// Cites: GEN-22, HAR-26
#[test]
fn gen_run_in_live_with_no_endpoint_serves_the_mock() {
    let dir = tempfile::tempdir().unwrap();
    // One session of one short turn: live waits on the wall clock.
    let sheet = SHEET
        .replace("sessions = 2", "sessions = 1")
        .replace(
            "turns_per_session = { const = 2 }",
            "turns_per_session = { const = 1 }",
        )
        .replace(
            "chain_length = { uniform = [1, 2] }",
            "chain_length = { const = 1 }",
        )
        .replace(
            "answer_tokens = { const = 16 }",
            "answer_tokens = { const = 4 }",
        );
    std::fs::write(dir.path().join("sheet.toml"), sheet).unwrap();
    let (code, j) = acn(
        dir.path(),
        &[
            "gen",
            "run",
            "--sheet",
            "sheet.toml",
            "--seed",
            "3",
            "--mode",
            "live",
            "--request-timeout-ms",
            "10000",
            "--max-retries",
            "0",
        ],
    );
    assert_eq!(code, Some(0), "{j}");
    assert_eq!(j["calls"], 2);
    let manifest: serde_json::Value = serde_json::from_slice(
        &std::fs::read(
            dir.path()
                .join(j["dir"].as_str().unwrap())
                .join("manifest.json"),
        )
        .unwrap(),
    )
    .unwrap();
    assert_eq!(manifest["endpoint_host"], "loopback");
}

/// Cites: GEN-22, HAR-26
#[test]
fn gen_run_refuses_the_served_mock_in_sim() {
    let dir = tempfile::tempdir().unwrap();
    std::fs::write(dir.path().join("sheet.toml"), SHEET).unwrap();
    let (code, j) = acn(
        dir.path(),
        &[
            "gen",
            "run",
            "--sheet",
            "sheet.toml",
            "--seed",
            "3",
            "--endpoint",
            "acn-mock://loopback",
        ],
    );
    assert_eq!(j["ok"], false, "{j}");
    assert_ne!(code, Some(0));
    assert!(j["error"].as_str().unwrap().contains("HAR-26"), "{j}");
}
