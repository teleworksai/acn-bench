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
    // Two sessions of two turns, each one or two tool calls and an answer.
    let calls = j["calls"].as_u64().unwrap();
    assert!((8..=12).contains(&calls), "{calls}");
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
}
