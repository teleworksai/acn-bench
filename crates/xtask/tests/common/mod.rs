//! Shared helpers for the xtask integration tests.
#![allow(dead_code, clippy::unwrap_used, clippy::expect_used, clippy::panic)]

use std::path::{Path, PathBuf};
use std::process::Command;

/// Result of one `xtask` invocation, with stdout already parsed under the
/// CON-8 contract (exactly one JSON object).
pub struct Run {
    pub code: Option<i32>,
    pub json: serde_json::Value,
    pub stderr: String,
}

impl Run {
    pub fn ok(&self) -> bool {
        self.json.get("ok").and_then(serde_json::Value::as_bool) == Some(true)
    }
}

/// The workspace root, derived from this crate's manifest directory.
pub fn repo_root() -> PathBuf {
    Path::new(env!("CARGO_MANIFEST_DIR"))
        .join("../..")
        .canonicalize()
        .expect("workspace root")
}

/// A fixture directory under `crates/xtask/tests/fixtures/`.
pub fn fixture(name: &str) -> PathBuf {
    Path::new(env!("CARGO_MANIFEST_DIR"))
        .join("tests/fixtures")
        .join(name)
}

/// Run the `xtask` binary with `args`, asserting the CON-8 stdout contract:
/// stdout parses as exactly one JSON object and the exit status is 0 iff
/// `"ok": true`.
pub fn xtask(args: &[&str]) -> Run {
    let out = Command::new(env!("CARGO_BIN_EXE_xtask"))
        .args(args)
        .output()
        .expect("spawn xtask");
    let stdout = String::from_utf8(out.stdout).expect("utf8 stdout");
    let stderr = String::from_utf8_lossy(&out.stderr).into_owned();
    let json: serde_json::Value = serde_json::from_str(stdout.trim()).unwrap_or_else(|e| {
        panic!(
            "stdout is not one JSON object: {e}\n--- stdout ---\n{stdout}\n--- stderr ---\n{stderr}"
        )
    });
    assert!(
        json.is_object(),
        "stdout must be a JSON object, got: {stdout}"
    );
    let ok = json.get("ok").and_then(serde_json::Value::as_bool) == Some(true);
    assert_eq!(
        out.status.code(),
        Some(if ok { 0 } else { 1 }),
        "exit code must be 0 iff ok:true\n--- stdout ---\n{stdout}\n--- stderr ---\n{stderr}"
    );
    Run {
        code: out.status.code(),
        json,
        stderr,
    }
}

/// Run `xtask` against a specific root directory.
pub fn xtask_at(root: &Path, args: &[&str]) -> Run {
    let root_s = root.to_str().expect("utf8 path");
    let mut full = vec!["--root", root_s];
    full.extend_from_slice(args);
    xtask(&full)
}

pub fn strings(v: &serde_json::Value, key: &str) -> Vec<String> {
    v.get(key)
        .and_then(serde_json::Value::as_array)
        .map(|a| {
            a.iter()
                .map(|x| match x {
                    serde_json::Value::String(s) => s.clone(),
                    other => other.to_string(),
                })
                .collect()
        })
        .unwrap_or_default()
}
