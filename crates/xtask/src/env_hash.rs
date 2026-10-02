//! `cargo xtask env-hash` (CON-7): a blake3 over the frozen set, recorded in
//! `env-hash.json` at the workspace root and checked as a gate (CON-9). See ADR-4.
//!
//! Two values are recorded (CON-28). `env_hash` covers the whole frozen set and is
//! what the CON-7 gate checks. `engine_hash` is the same construction over the
//! frozen paths under `crates/` only, and is what runs, bundles and verdicts are
//! bound to: a run's hypothesis, scenario and workload enter `run_id` through
//! their own hashes, so freezing another POC's hypothesis must not change it.
//!
//! The walk fails closed: every entry under a frozen directory is either a
//! regular file that gets hashed, a directory, or an error. Nothing is skipped
//! by name except a zero-length `.gitkeep`.

use std::path::Path;

use serde::Serialize;

pub use acn_trace::env::{ENGINE_PREFIX, FROZEN_SET, FileHash, RECORD_FILE};

use crate::workspace::write;
use crate::{Error, Result};

/// The recorded (and computed) environment hash. The walk and both hashes live in
/// `acn_trace::env`, so the gate and the check every run makes (CON-28) read the
/// same bytes the same way.
pub type EnvHash = acn_trace::env::EnvRecord;

fn env_err(e: acn_trace::env::EnvError) -> Error {
    Error::Invalid(e.to_string())
}

/// Per-file difference between the computed and the recorded set.
#[derive(Debug, Default, Serialize)]
pub struct Diff {
    pub added: Vec<String>,
    pub removed: Vec<String>,
    pub changed: Vec<String>,
}

/// The JSON object `env-hash` prints (CON-8).
#[derive(Debug, Serialize)]
pub struct Report {
    pub ok: bool,
    pub mode: &'static str,
    pub env_hash: String,
    pub engine_hash: String,
    pub recorded: Option<String>,
    pub record_file: String,
    #[serde(skip_serializing_if = "Option::is_none")]
    pub diff: Option<Diff>,
    #[serde(skip_serializing_if = "Option::is_none")]
    pub error: Option<String>,
    pub files: Vec<FileHash>,
}

fn hash_of(files: &[FileHash]) -> String {
    acn_trace::env::record_hash(files).to_hex()
}

fn engine_hash_of(files: &[FileHash]) -> String {
    acn_trace::env::engine_hash_of(files).to_hex()
}

/// Compute the hash over the frozen set under `root`.
pub fn compute(root: &Path) -> Result<EnvHash> {
    acn_trace::env::compute(root).map_err(env_err)
}

/// Read the recorded hash, if any.
pub fn recorded(root: &Path) -> Result<Option<EnvHash>> {
    acn_trace::env::read_record(root).map_err(env_err)
}

fn diff(computed: &EnvHash, recorded: &EnvHash) -> Diff {
    let mut d = Diff::default();
    for f in &computed.files {
        match recorded.files.iter().find(|r| r.path == f.path) {
            None => d.added.push(f.path.clone()),
            Some(r) if r.blake3 != f.blake3 => d.changed.push(f.path.clone()),
            Some(_) => {}
        }
    }
    for r in &recorded.files {
        if !computed.files.iter().any(|f| f.path == r.path) {
            d.removed.push(r.path.clone());
        }
    }
    d
}

/// What to do.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum Mode {
    Print,
    Check,
    Write,
}

/// Run in `mode`.
pub fn run(root: &Path, mode: Mode) -> Result<Report> {
    let computed = compute(root)?;
    // Write mode must be able to repair a corrupt record, so it does not parse the old
    // one. Print mode computes and reports; a record it cannot parse (one written
    // before `engine_hash` existed, say) is shown as absent. Only the gate insists.
    let rec = match mode {
        Mode::Write => None,
        Mode::Print => recorded(root).ok().flatten(),
        Mode::Check => recorded(root)?,
    };
    let recorded_hash = rec.as_ref().map(|r| r.env_hash.clone());
    let mut report = Report {
        ok: true,
        mode: match mode {
            Mode::Print => "print",
            Mode::Check => "check",
            Mode::Write => "write",
        },
        env_hash: computed.env_hash.clone(),
        engine_hash: computed.engine_hash.clone(),
        recorded: recorded_hash.clone(),
        record_file: RECORD_FILE.to_owned(),
        diff: None,
        error: None,
        files: computed.files.clone(),
    };
    match mode {
        Mode::Print => {}
        Mode::Check => {
            // The whole record must match, not only the top-level hash: the per-file
            // list is what a reviewer reads to see which frozen file moved, and it
            // must itself hash to the recorded value.
            let hint = match &rec {
                None => Some(format!(
                    "{RECORD_FILE} is missing: run `cargo xtask env-hash --write`"
                )),
                Some(r) if hash_of(&r.files) != r.env_hash => Some(format!(
                    "{RECORD_FILE} is inconsistent: its `env_hash` is not the hash of its own `files` list; regenerate it with `cargo xtask env-hash --write`"
                )),
                Some(r) if engine_hash_of(&r.files) != r.engine_hash => Some(format!(
                    "{RECORD_FILE} is inconsistent: its `engine_hash` is not the hash of its own frozen-crate entries (CON-28); regenerate it with `cargo xtask env-hash --write`"
                )),
                Some(r) if *r != computed => Some(format!(
                    "frozen set differs from {RECORD_FILE}: if the change is intended, run `cargo xtask env-hash --write` and label the PR `env-change` (CON-7)"
                )),
                Some(_) => None,
            };
            if let Some(hint) = hint {
                report.ok = false;
                report.diff = rec.as_ref().map(|r| diff(&computed, r));
                tracing::error!(
                    computed = %computed.env_hash,
                    recorded = recorded_hash.as_deref().unwrap_or("<none>"),
                    "{hint}"
                );
                report.error = Some(hint);
            }
        }
        Mode::Write => {
            let mut text = serde_json::to_string_pretty(&computed)?;
            text.push('\n');
            write(&root.join(RECORD_FILE), &text)?;
        }
    }
    Ok(report)
}
