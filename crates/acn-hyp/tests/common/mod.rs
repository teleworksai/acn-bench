//! Helpers shared by the acn-hyp tests: a minimal valid file, and loading text as
//! a candidate or as a frozen file under a temporary workspace root.
#![allow(dead_code, clippy::unwrap_used, clippy::expect_used, clippy::panic)]

use std::path::{Path, PathBuf};

use acn_hyp::{HypError, Hypothesis};

/// A minimal valid candidate: two pooled parameters (four cells), a control.
pub const BASE: &str = r#"[poc]
id = "t1"
title = "a test"

[hypothesis]
statement = "s"

[varies]
knob = { kind = "bool" }
mode = { kind = "enum", values = ["fast", "slow"] }

[measures]
primary = ["cached_token_ratio"]
secondary = ["ttft_p50_ms", "ttft_p99_ms"]

[control]
description = "defaults"
config = { knob = false }

[design]
search = "grid"
replicates = 4
twin_required = false

[falsifier]
predicate = "max_over_knobs(abs(effect(cached_token_ratio))) < noise_floor(cached_token_ratio, control)"

[expected]
outcome = "pass"
"#;

/// The file with its predicate replaced.
pub fn with_predicate(p: &str) -> String {
    BASE.replace(
        "predicate = \"max_over_knobs(abs(effect(cached_token_ratio))) < noise_floor(cached_token_ratio, control)\"",
        &format!("predicate = {}", toml_string(p)),
    )
}

pub fn toml_string(s: &str) -> String {
    format!("\"{}\"", s.replace('\\', "\\\\").replace('"', "\\\""))
}

/// Load `text` as a candidate named `<stem>.toml`, outside any workspace.
pub fn candidate(text: &str, stem: &str) -> (Result<Hypothesis, HypError>, tempfile::TempDir) {
    let dir = tempfile::tempdir().unwrap();
    let path = dir.path().join(format!("{stem}.toml"));
    std::fs::write(&path, text).unwrap();
    (acn_hyp::load(&path), dir)
}

/// A temporary workspace root holding `files` (relative path, text) under it,
/// `specs/README.md` listing `specs/100-x.md`, and an `env-hash.json` that records
/// every file under `hypotheses/` with its hash.
pub fn root(files: &[(&str, &str)]) -> tempfile::TempDir {
    let dir = tempfile::tempdir().unwrap();
    let r = dir.path();
    std::fs::create_dir_all(r.join("hypotheses")).unwrap();
    std::fs::create_dir_all(r.join("specs")).unwrap();
    std::fs::write(
        r.join("specs/README.md"),
        "| 100 | 100-x.md | X | to write |\n",
    )
    .unwrap();
    let mut recorded = Vec::new();
    for (rel, text) in files {
        let p = r.join(rel);
        std::fs::create_dir_all(p.parent().unwrap()).unwrap();
        std::fs::write(&p, text).unwrap();
        if rel.starts_with("hypotheses/") {
            recorded.push(serde_json::json!({ "path": rel, "blake3": blake3::hash(text.as_bytes()).to_hex().to_string() }));
        }
    }
    let record = serde_json::json!({ "env_hash": "0".repeat(64), "engine_hash": "0".repeat(64), "files": recorded });
    std::fs::write(r.join("env-hash.json"), record.to_string()).unwrap();
    dir
}

/// The file made frozen-shaped: 20 replicates and a spec.
pub fn frozen_text(text: &str) -> String {
    text.replace("replicates = 4", "replicates = 20").replace(
        "title = \"a test\"",
        "title = \"a test\"\nspec = \"specs/100-x.md\"",
    )
}

/// Load `text` frozen: as `hypotheses/<stem>.toml` under a root that records it.
pub fn frozen(text: &str, stem: &str) -> (Result<Hypothesis, HypError>, tempfile::TempDir) {
    let rel = format!("hypotheses/{stem}.toml");
    let dir = root(&[(&rel, text)]);
    let path: PathBuf = dir.path().join(&rel);
    (acn_hyp::load(&path), dir)
}

/// The error message of a load that must fail.
pub fn err(r: Result<Hypothesis, HypError>) -> String {
    match r {
        Ok(h) => panic!("loaded {}, expected an error", h.path.display()),
        Err(e) => e.to_string(),
    }
}

pub fn exists(p: &Path) -> bool {
    p.exists()
}
