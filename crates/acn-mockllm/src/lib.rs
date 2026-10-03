//! `acn-mockllm` — a deterministic, OpenAI-compatible mock inference server
//! (SPEC 030). One rule set: [`engine::Mock`] answers a request as a pure function
//! of its state, for in-process `sim` use, and [`server`] serves the same over
//! HTTP for `live`, emitting each token at its stated time on the run's clock.
//! Bundles made against it are mock-gated (CON-26).
#![forbid(unsafe_code)]

pub mod cache;
pub mod engine;
pub mod profile;
pub mod prompt;
pub mod server;

pub use engine::{Mock, Outcome};

/// Anything that stops the mock from starting or serving.
#[derive(Debug, thiserror::Error)]
pub enum MockError {
    #[error(transparent)]
    Profile(#[from] profile::ProfileError),
    #[error(transparent)]
    Identity(#[from] acn_trace::identity::IdentityError),
    #[error("io: {0}")]
    Io(#[from] std::io::Error),
}
