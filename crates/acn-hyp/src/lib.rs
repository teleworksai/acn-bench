//! `acn-hyp` — hypothesis files and the falsifier language (SPEC 080); verdicts
//! (T05.2) and the loop runner (SPEC 085) will follow. Frozen set (CON-7): every
//! change here is a Class C change. T05 lands in parts (ADR-18): this crate now
//! holds the file format (HYP-1..9), the predicate language, its static checks
//! and its one evaluator (HYP-10..14), the quantity table (HYP-12) and lint
//! (HYP-27).
#![forbid(unsafe_code)]

use std::path::{Path, PathBuf};

pub mod check;
pub mod eval;
pub mod file;
pub mod lint;
pub mod predicate;
pub mod quantities;

pub use file::{Hypothesis, load, load_in};

/// A hypothesis file's status (HYP-3).
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum Status {
    Frozen,
    Candidate,
}

impl Status {
    #[must_use]
    pub fn as_str(self) -> &'static str {
        match self {
            Self::Frozen => "frozen",
            Self::Candidate => "candidate",
        }
    }
}

/// A file that cannot be used: where, at which key, and why (HYP-1).
#[derive(Debug, Clone, PartialEq, Eq, thiserror::Error)]
#[error("{}: {}{message}", path.display(), key.as_deref().map(|k| format!("{k}: ")).unwrap_or_default())]
pub struct HypError {
    pub path: PathBuf,
    /// The key path, such as `design.replicates`.
    pub key: Option<String>,
    pub message: String,
}

impl HypError {
    #[must_use]
    pub fn new(path: &Path, key: Option<String>, message: String) -> Self {
        Self {
            path: path.to_path_buf(),
            key,
            message,
        }
    }
}
