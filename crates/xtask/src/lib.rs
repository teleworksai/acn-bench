//! `xtask` — repository automation for acn-bench.
//!
//! Three tasks, each a CON-8 command: `trace-check` (CON-12), `docs-inventory`
//! (CON-9, later TRC-20 and LOOP-30) and `env-hash` (CON-7). The library holds
//! the logic so it can be unit-tested; `main.rs` is the thin CLI.
#![forbid(unsafe_code)]

pub mod attributes;
pub mod citations;
pub mod docs_inventory;
pub mod env_hash;
pub mod error;
pub mod logging;
pub mod model;
pub mod pr_check;
pub mod refs;
pub mod scope;
pub mod specs;
pub mod trace_check;
pub mod workspace;

pub use error::Error;

/// Result alias for the xtask library.
pub type Result<T> = std::result::Result<T, Error>;
