//! `acn-trace` — trace schema, Arrow/Parquet IO, run bundle and manifest (SPEC 010).
//!
//! T02 lands as a series of PRs (ADR-11). This is T02a: the frozen schema module
//! and the provider normalisation that reads it.
#![forbid(unsafe_code)]

pub mod normalise;
pub mod schema;
