//! `acn-harness` — a minimal agent harness with switchable cache-discipline knobs
//! (SPEC 040). It does to a model's context what real harnesses do — a system
//! prompt, tool definitions, tool results, sub-agents, compaction — with each
//! cache-relevant habit a named knob (HAR-10), against the mock in process
//! (`sim`) or any endpoint over HTTP (`live`), and records what each habit costs
//! in the trace schema of SPEC 010.
#![forbid(unsafe_code)]

pub mod agent;
pub mod context;
pub mod credentials;
pub mod env;
pub mod knobs;
pub mod run;
pub mod wire;
pub mod workload;

/// Why the harness could not run.
#[derive(Debug, thiserror::Error)]
pub enum HarnessError {
    #[error("workload: {0}")]
    Workload(String),
    #[error("knob: {0}")]
    Knob(String),
    #[error("config: {0}")]
    Config(String),
    /// HAR-23: the endpoint is not the backend the run was configured with.
    #[error("backend_mismatch: {0}")]
    BackendMismatch(String),
    #[error("backend: {0}")]
    Backend(String),
    #[error(transparent)]
    Identity(#[from] acn_trace::identity::IdentityError),
    #[error(transparent)]
    Env(#[from] acn_trace::env::EnvError),
    #[error(transparent)]
    Bundle(#[from] acn_trace::bundle::BundleError),
    #[error(transparent)]
    Schema(#[from] acn_trace::schema::SchemaError),
    #[error(transparent)]
    Convert(#[from] acn_trace::otel::ConvertError),
    #[error("internal: {0}")]
    Internal(String),
}
