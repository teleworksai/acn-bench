//! `acn-trace` — trace schema, Arrow/Parquet IO, run bundle and manifest (SPEC 010).
//!
//! T02 lands as a series of PRs (ADR-11). T02a added the frozen schema module and the
//! provider normalisation that reads it; T02b adds run identity, the environment and
//! build hashes, and (feature `io`) the span model, the Parquet writer, the seeded
//! id generator and the bundle.
#![forbid(unsafe_code)]

pub mod env;
pub mod identity;
pub mod normalise;
pub mod schema;

#[cfg(feature = "io")]
pub mod bundle;
#[cfg(feature = "io")]
pub mod fixture;
#[cfg(feature = "io")]
pub mod ids;
#[cfg(feature = "io")]
pub mod ingest;
#[cfg(feature = "io")]
pub mod model;
#[cfg(feature = "io")]
pub mod otel;
#[cfg(feature = "io")]
pub mod parquet_io;
