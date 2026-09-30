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

use serde::{Deserialize, Serialize};
use walkdir::WalkDir;

use crate::workspace::{read, rel_strict, write};
use crate::{Error, Result};

/// The frozen set (CON-7), relative to the workspace root.
pub const FROZEN_SET: &[&str] = &[
    "hypotheses",
    "scenarios/measured",
    "crates/acn-hyp",
    "crates/acn-attrib/src/core",
    "crates/acn-trace/src/schema",
];

/// The part of the frozen set that is code: the verdict engine, the attribution
/// core and the trace schema. `engine_hash` covers exactly the files below it.
pub const ENGINE_PREFIX: &str = "crates/";

/// Where the recorded hash lives, relative to the workspace root.
pub const RECORD_FILE: &str = "env-hash.json";

/// One hashed file.
#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
#[serde(deny_unknown_fields)]
pub struct FileHash {
    pub path: String,
    pub blake3: String,
}

/// The recorded (and computed) environment hash.
#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
#[serde(deny_unknown_fields)]
pub struct EnvHash {
    pub env_hash: String,
    /// Required: a record written before CON-28 fails to parse, and the error
    /// says how to regenerate it.
    pub engine_hash: String,
    pub files: Vec<FileHash>,
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

/// blake3 of a file, streamed so a large measured trace cannot exhaust memory.
fn hash_file(path: &Path) -> Result<String> {
    let file = std::fs::File::open(path).map_err(|e| Error::io(path, e))?;
    let mut hasher = blake3::Hasher::new();
    hasher.update_reader(file).map_err(|e| Error::io(path, e))?;
    Ok(hasher.finalize().to_hex().to_string())
}

/// The environment hash of a file list: blake3 over `path \0 hex \n` records.
fn hash_of(files: &[FileHash]) -> String {
    let mut hasher = blake3::Hasher::new();
    for f in files {
        hasher.update(f.path.as_bytes());
        hasher.update(b"\0");
        hasher.update(f.blake3.as_bytes());
        hasher.update(b"\n");
    }
    hasher.finalize().to_hex().to_string()
}

/// `engine_hash` of a file list: the record hash of its frozen-crate entries, in
/// the order of the list (which is sorted by path).
fn engine_hash_of(files: &[FileHash]) -> String {
    let engine: Vec<FileHash> = files
        .iter()
        .filter(|f| f.path.starts_with(ENGINE_PREFIX))
        .cloned()
        .collect();
    hash_of(&engine)
}

/// Compute the hash over the frozen set under `root`.
pub fn compute(root: &Path) -> Result<EnvHash> {
    if !root.is_dir() {
        return Err(Error::Invalid(format!(
            "--root {} is not a directory",
            root.display()
        )));
    }
    let mut files = Vec::new();
    for base in FROZEN_SET {
        let dir = root.join(base);
        // symlink_metadata: a dangling or redirecting symlink in place of a frozen
        // directory must be refused, not skipped as "does not exist".
        let meta = std::fs::symlink_metadata(&dir).map_err(|e| {
            Error::Invalid(format!(
                "frozen directory `{base}` is missing under {} ({e}); the frozen set is fixed by CON-7",
                root.display()
            ))
        })?;
        if !meta.is_dir() {
            return Err(Error::Invalid(format!(
                "frozen path `{base}` is not a real directory"
            )));
        }
        for entry in WalkDir::new(&dir).sort_by_file_name() {
            let entry = entry?;
            let ty = entry.file_type();
            let shown = entry.path().display();
            if ty.is_dir() {
                continue;
            }
            if !ty.is_file() {
                return Err(Error::Invalid(format!(
                    "only regular files are allowed in the frozen set; found a symlink or special file: {shown}"
                )));
            }
            let name = entry.file_name().to_str().unwrap_or("");
            let len = entry.metadata()?.len();
            if name == ".gitkeep" && len == 0 {
                continue;
            }
            if name == ".DS_Store" {
                return Err(Error::Invalid(format!(
                    "{shown}: remove Finder metadata from the frozen set (`find . -name .DS_Store -delete`)"
                )));
            }
            files.push(FileHash {
                path: rel_strict(root, entry.path())?,
                blake3: hash_file(entry.path())?,
            });
        }
    }
    files.sort_by(|a, b| a.path.cmp(&b.path));
    Ok(EnvHash {
        env_hash: hash_of(&files),
        engine_hash: engine_hash_of(&files),
        files,
    })
}

/// Read the recorded hash, if any.
pub fn recorded(root: &Path) -> Result<Option<EnvHash>> {
    let path = root.join(RECORD_FILE);
    if !path.is_file() {
        return Ok(None);
    }
    let parsed: EnvHash = serde_json::from_str(&read(&path)?).map_err(|e| {
        Error::Invalid(format!(
            "{RECORD_FILE} is not a valid env-hash record ({e}); it must carry `env_hash`, `engine_hash` (CON-28) and `files`; regenerate it with `cargo xtask env-hash --write` in an `env-change` PR"
        ))
    })?;
    Ok(Some(parsed))
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
