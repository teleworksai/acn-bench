//! `acn-ctl` — the control plane (SPEC 070): a registry of run requests on
//! disk (CTL-10 to CTL-13) and the worker that runs them, one at a time,
//! through the same run paths as the CLI. It decides nothing a run records
//! (ADR-39).
#![forbid(unsafe_code)]

pub mod api;
pub mod registry;
pub mod request;
pub mod resolve;
pub mod server;

pub use registry::{Ctl, CtlConfig, Outcome, State, Status, Submitted};
pub use request::{Kind, ReqOpts, ScenarioRef, Submit};

/// A refused API call (CTL-2): its HTTP status, its code and what is wrong.
#[derive(Debug, Clone, PartialEq, Eq, thiserror::Error)]
#[error("{code}: {error}")]
pub struct Refusal {
    pub status: u16,
    pub code: &'static str,
    pub error: String,
}

impl Refusal {
    pub fn new(status: u16, code: &'static str, error: impl Into<String>) -> Self {
        Self {
            status,
            code,
            error: error.into(),
        }
    }

    pub fn bad(code: &'static str, error: impl Into<String>) -> Self {
        Self::new(400, code, error)
    }

    pub fn internal(error: impl std::fmt::Display) -> Self {
        Self::new(500, "internal", error.to_string())
    }
}
