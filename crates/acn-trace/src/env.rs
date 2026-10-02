//! The workspace root and the hashes that bind a run to its environment: the
//! frozen-set record of ADR-4 (`env_hash`, `engine_hash`, CON-28), the tree hash of
//! CON-27(b) that `source_hash` (CON-31) is built from, and the preflight every run
//! passes before it may write a bundle.
//!
//! `cargo xtask env-hash` and the run path read the same walk, so the gate and the
//! check a run makes cannot disagree about which bytes are frozen.

use std::path::{Path, PathBuf};

use serde::{Deserialize, Serialize};
use walkdir::WalkDir;

use crate::identity::{Digest, HypStatus, Preimage};

/// The frozen set (CON-7), relative to the workspace root.
pub const FROZEN_SET: &[&str] = &[
    "hypotheses",
    "scenarios/measured",
    "crates/acn-hyp",
    "crates/acn-attrib/src/core",
    "crates/acn-trace/src/schema",
];

/// The part of the frozen set that is code: `engine_hash` covers exactly the
/// frozen files below it (CON-28).
pub const ENGINE_PREFIX: &str = "crates/";

/// The record, relative to the workspace root; its presence marks the root.
pub const RECORD_FILE: &str = "env-hash.json";

/// An environment or tree that cannot be hashed, or a run that must not start.
#[derive(Debug, thiserror::Error)]
pub enum EnvError {
    #[error("io error at {path}: {source}")]
    Io {
        path: PathBuf,
        source: std::io::Error,
    },
    #[error("directory walk failed: {0}")]
    Walk(#[from] walkdir::Error),
    #[error("{0}")]
    Invalid(String),
    /// CON-28: the run must not start.
    #[error("refusing to start: {0}")]
    Refused(String),
}

type Result<T> = std::result::Result<T, EnvError>;

fn io(path: &Path) -> impl FnOnce(std::io::Error) -> EnvError + '_ {
    move |source| EnvError::Io {
        path: path.to_path_buf(),
        source,
    }
}

/// One hashed file: its path relative to the root, with `/` separators, and the
/// BLAKE3 of its bytes as lowercase hex.
#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
#[serde(deny_unknown_fields)]
pub struct FileHash {
    pub path: String,
    pub blake3: String,
}

/// The content of `env-hash.json` (ADR-4, CON-28).
#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
#[serde(deny_unknown_fields)]
pub struct EnvRecord {
    pub env_hash: String,
    /// Required: a record written before CON-28 fails to parse.
    pub engine_hash: String,
    pub files: Vec<FileHash>,
}

/// The ADR-4 record bytes: for each file, the path, one zero byte, the file hash as
/// 64 lowercase hex characters, one `\n`.
fn records(files: &[FileHash]) -> Vec<u8> {
    let mut out = Vec::new();
    for f in files {
        out.extend_from_slice(f.path.as_bytes());
        out.push(0);
        out.extend_from_slice(f.blake3.as_bytes());
        out.push(b'\n');
    }
    out
}

/// `env_hash` construction of ADR-4: plain BLAKE3 over the records, un-prefixed.
#[must_use]
pub fn record_hash(files: &[FileHash]) -> Digest {
    Digest::of(&records(files))
}

/// `engine_hash` of a file list: the record hash of its entries under
/// [`ENGINE_PREFIX`], in list order (which is sorted by path).
#[must_use]
pub fn engine_hash_of(files: &[FileHash]) -> Digest {
    let engine: Vec<FileHash> = files
        .iter()
        .filter(|f| f.path.starts_with(ENGINE_PREFIX))
        .cloned()
        .collect();
    record_hash(&engine)
}

/// The tree hash of CON-27(b): `blake3("acn-bench/tree/v1\0" ‖ records)`, the records
/// written as ADR-4 writes them. `files` must be sorted bytewise by path.
pub fn tree_hash(files: &[FileHash]) -> Result<Digest> {
    if files.windows(2).any(|w| w[0].path >= w[1].path) {
        return Err(EnvError::Invalid(
            "a tree hash is over files sorted bytewise by path, each once".into(),
        ));
    }
    // The records are written raw: this is the one preimage in which a digest is
    // hex rather than raw bytes (CON-27(b)).
    let p = Preimage::new("acn-bench/tree/v1").map_err(|e| EnvError::Invalid(e.to_string()))?;
    let mut bytes = p.bytes().to_vec();
    bytes.extend_from_slice(&records(files));
    Ok(Digest::of(&bytes))
}

/// A path relative to `root` with `/` separators; a name that is not UTF-8 is an
/// error, so that two different names are never recorded as the same string.
pub fn rel_path(root: &Path, path: &Path) -> Result<String> {
    let p = path.strip_prefix(root).unwrap_or(path);
    let mut parts = Vec::new();
    for c in p.components() {
        let s = c.as_os_str().to_str().ok_or_else(|| {
            EnvError::Invalid(format!(
                "path is not valid UTF-8 and cannot be recorded: {}",
                path.display()
            ))
        })?;
        parts.push(s.to_owned());
    }
    Ok(parts.join("/"))
}

fn hash_hex(path: &Path) -> Result<String> {
    crate::identity::file_hash(path)
        .map(|d| d.to_hex())
        .map_err(|e| EnvError::Invalid(e.to_string()))
}

/// Walk `dir` with the rules of ADR-4: every entry is a regular file that is hashed,
/// a directory, or an error. A symbolic link, a special file or a `.DS_Store` is an
/// error; a zero-length `.gitkeep` is skipped. `skip_dir` prunes directories (the
/// source tree leaves out `crates/<crate>/target`).
fn walk_into(
    root: &Path,
    dir: &Path,
    skip_dir: &dyn Fn(&str) -> bool,
    out: &mut Vec<FileHash>,
) -> Result<()> {
    let mut it = WalkDir::new(dir).sort_by_file_name().into_iter();
    while let Some(entry) = it.next() {
        let entry = entry?;
        let ty = entry.file_type();
        let shown = entry.path().display();
        if ty.is_dir() {
            if entry.depth() > 0 && skip_dir(&rel_path(root, entry.path())?) {
                it.skip_current_dir();
            }
            continue;
        }
        if !ty.is_file() {
            return Err(EnvError::Invalid(format!(
                "only regular files are allowed in the frozen set; found a symlink or special file: {shown}"
            )));
        }
        let name = entry.file_name().to_str().unwrap_or("");
        let len = entry.metadata()?.len();
        if name == ".gitkeep" && len == 0 {
            continue;
        }
        if name == ".DS_Store" {
            return Err(EnvError::Invalid(format!(
                "{shown}: remove Finder metadata from the frozen set (`find . -name .DS_Store -delete`)"
            )));
        }
        out.push(FileHash {
            path: rel_path(root, entry.path())?,
            blake3: hash_hex(entry.path())?,
        });
    }
    Ok(())
}

/// A base directory that must exist as a real directory.
fn real_dir(root: &Path, base: &str, what: &str) -> Result<PathBuf> {
    let dir = root.join(base);
    // symlink_metadata: a dangling or redirecting symlink in place of a directory
    // must be refused, not skipped as "does not exist".
    let meta = std::fs::symlink_metadata(&dir).map_err(|e| {
        EnvError::Invalid(format!(
            "{what} `{base}` is missing under {} ({e}); the frozen set is fixed by CON-7",
            root.display()
        ))
    })?;
    if !meta.is_dir() {
        return Err(EnvError::Invalid(format!(
            "{what} `{base}` is not a real directory"
        )));
    }
    Ok(dir)
}

/// Hash the frozen set under `root` (ADR-4, CON-28).
pub fn compute(root: &Path) -> Result<EnvRecord> {
    if !root.is_dir() {
        return Err(EnvError::Invalid(format!(
            "--root {} is not a directory",
            root.display()
        )));
    }
    let mut files = Vec::new();
    for base in FROZEN_SET {
        let dir = real_dir(root, base, "frozen directory")?;
        walk_into(root, &dir, &|_| false, &mut files)?;
    }
    files.sort_by(|a, b| a.path.cmp(&b.path));
    Ok(EnvRecord {
        env_hash: record_hash(&files).to_hex(),
        engine_hash: engine_hash_of(&files).to_hex(),
        files,
    })
}

/// The files `source_hash` covers (CON-31): every file under `crates/`, except a
/// `target` directory that is a direct child of a crate directory, plus the root
/// `Cargo.toml`; sorted bytewise by path.
pub fn source_files(root: &Path) -> Result<Vec<FileHash>> {
    let dir = real_dir(root, "crates", "source directory")?;
    let mut files = Vec::new();
    let crate_target = |rel: &str| {
        let parts: Vec<&str> = rel.split('/').collect();
        parts.len() == 3 && parts[0] == "crates" && parts[2] == "target"
    };
    walk_into(root, &dir, &crate_target, &mut files)?;
    let manifest = root.join("Cargo.toml");
    let meta = std::fs::symlink_metadata(&manifest).map_err(io(&manifest))?;
    if !meta.is_file() {
        return Err(EnvError::Invalid(
            "the root Cargo.toml must be a regular file".into(),
        ));
    }
    files.push(FileHash {
        path: "Cargo.toml".into(),
        blake3: hash_hex(&manifest)?,
    });
    files.sort_by(|a, b| a.path.cmp(&b.path));
    Ok(files)
}

/// Read `env-hash.json` under `root`, if present.
pub fn read_record(root: &Path) -> Result<Option<EnvRecord>> {
    let path = root.join(RECORD_FILE);
    if !path.is_file() {
        return Ok(None);
    }
    let text = std::fs::read_to_string(&path).map_err(io(&path))?;
    let parsed: EnvRecord = serde_json::from_str(&text).map_err(|e| {
        EnvError::Invalid(format!(
            "{RECORD_FILE} is not a valid env-hash record ({e}); it must carry `env_hash`, `engine_hash` (CON-28) and `files`; regenerate it with `cargo xtask env-hash --write` in an `env-change` PR"
        ))
    })?;
    Ok(Some(parsed))
}

/// The workspace root of CON-28: the nearest ancestor of `start` (itself included)
/// that holds `env-hash.json`.
#[must_use]
pub fn find_root(start: &Path) -> Option<PathBuf> {
    start
        .ancestors()
        .find(|dir| dir.join(RECORD_FILE).is_file())
        .map(Path::to_path_buf)
}

/// The hypothesis a run is about to use, as the preflight needs to know it.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum RunHypothesis {
    /// No hypothesis (fixtures, lab runs, researcher sessions; CON-27(a)).
    None,
    Status(HypStatus),
}

/// Proof that a run passed the CON-28 checks. The bundle writer takes one, so a run
/// cannot write a bundle without having made them.
#[derive(Debug, Clone)]
pub struct Preflight {
    root: Option<PathBuf>,
    engine_hash: Digest,
    hypothesis: RunHypothesis,
}

impl Preflight {
    /// The workspace root the run was checked against, if one was found.
    #[must_use]
    pub fn root(&self) -> Option<&Path> {
        self.root.as_deref()
    }

    /// The `engine_hash` the run is bound to.
    #[must_use]
    pub fn engine_hash(&self) -> Digest {
        self.engine_hash
    }

    /// The hypothesis the check was made for.
    #[must_use]
    pub fn hypothesis(&self) -> RunHypothesis {
        self.hypothesis
    }
}

/// CON-28: find the root from `start`; when one is found, recompute both hashes and
/// refuse if either differs from the record, or if `engine_hash` differs from the
/// value `embedded` in the binary at compile time. When none is found, refuse unless
/// the hypothesis is `none` or a candidate; `engine_hash` is then `embedded`.
pub fn preflight(start: &Path, embedded: Digest, hypothesis: RunHypothesis) -> Result<Preflight> {
    let Some(root) = find_root(start) else {
        return match hypothesis {
            RunHypothesis::Status(HypStatus::Frozen) => Err(EnvError::Refused(format!(
                "no {RECORD_FILE} above {}: a run with a frozen hypothesis needs a workspace root or a kit (CON-28)",
                start.display()
            ))),
            _ => Ok(Preflight {
                root: None,
                engine_hash: embedded,
                hypothesis,
            }),
        };
    };
    let Some(record) = read_record(&root)? else {
        return Err(EnvError::Refused(format!(
            "{RECORD_FILE} under {} vanished during the check",
            root.display()
        )));
    };
    let computed = compute(&root)?;
    if computed.env_hash != record.env_hash || computed.files != record.files {
        return Err(EnvError::Refused(format!(
            "the frozen set under {} differs from {RECORD_FILE} (computed env_hash {}, recorded {}); run `cargo xtask env-hash --check` to see which file moved (CON-28)",
            root.display(),
            computed.env_hash,
            record.env_hash
        )));
    }
    if computed.engine_hash != record.engine_hash {
        return Err(EnvError::Refused(format!(
            "engine_hash {} differs from {RECORD_FILE} ({}) (CON-28)",
            computed.engine_hash, record.engine_hash
        )));
    }
    let engine_hash =
        Digest::from_hex(&computed.engine_hash).map_err(|e| EnvError::Invalid(e.to_string()))?;
    if engine_hash != embedded {
        return Err(EnvError::Refused(format!(
            "this binary was built from frozen code with engine_hash {embedded}, but the checkout at {} has {engine_hash}; rebuild (CON-28, CON-31)",
            root.display()
        )));
    }
    Ok(Preflight {
        root: Some(root),
        engine_hash,
        hypothesis,
    })
}
