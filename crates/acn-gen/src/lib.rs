//! `acn-gen` — the workload generator (SPEC 050): a sheet of distributions
//! (GEN-1, GEN-2) from which seeded sessions, turns and calls are drawn
//! (GEN-3, GEN-4), run on the harness's run path (GEN-20).
#![forbid(unsafe_code)]

pub mod plan;
pub mod sheet;
pub mod text;

/// Why a sheet or a plan could not be made.
#[derive(Debug, thiserror::Error)]
pub enum GenError {
    /// A sheet that is refused at load (GEN-1, GEN-2), naming the parameter.
    #[error("sheet: {0}")]
    Sheet(String),
    #[error("internal: {0}")]
    Internal(String),
}
