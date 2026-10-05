//! `acn-accept` — shared helpers for the acceptance suites in `tests/accept/<poc>.rs`.
//!
//! Each suite cites its POC spec IDs and the hypothesis file it instantiates
//! (CON-12) and runs the control named there (CON-18).
#![forbid(unsafe_code)]

use acn_trace::identity::{BuildInfo, BuildParts, Digest};

/// A build identity for tests (CON-31), its source hash taken from `tag`.
///
/// # Panics
/// Never: the parts are fixed and valid.
#[must_use]
#[allow(clippy::expect_used)] // CON-19: a test helper with fixed inputs
pub fn build(tag: &str) -> BuildInfo {
    BuildParts {
        cargo_lock: Digest::of(b"lock"),
        rust_toolchain: Digest::of(b"toolchain"),
        cargo_config: Digest::of(b"config"),
        source_hash: Digest::of(tag.as_bytes()),
        target: "accept",
        profile: "debug",
        features: "",
        rustflags: "",
    }
    .info()
    .expect("fixed build parts are valid")
}
