//! `acn-hyp` — hypothesis files, the falsifier language and verdicts (SPEC 080);
//! the loop runner (SPEC 085) will follow. Frozen set
//! (CON-7): every change here is a Class C change. T05 landed in parts (ADR-18 to
//! ADR-21): this crate holds the file format (HYP-1..9), the
//! predicate language, its static checks and its one evaluator (HYP-10..14), the
//! quantity table with its formulas and prices (HYP-12), the bootstrap (HYP-13,
//! HYP-15), evaluation over a slice's data (HYP-11, HYP-14), bundles read into a
//! verdict and `verdict.json` (HYP-20..24, HYP-28), lint (HYP-27), and the
//! read-only rules (HYP-4, HYP-25): it opens hypothesis files only to read them,
//! writes nothing but verdicts under `runs/`, and detects a file that changed
//! under it.
#![forbid(unsafe_code)]

use std::path::{Path, PathBuf};

pub mod bootstrap;
pub mod check;
pub mod eval;
pub mod file;
pub mod json;
pub mod lint;
pub mod predicate;
pub mod quantities;
pub mod read;
pub mod slice;
pub mod verdict;

pub use file::{HYPOTHESIS_CHANGED, Hypothesis, load, load_in};

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
