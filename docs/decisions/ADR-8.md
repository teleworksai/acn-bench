# ADR-8 — Determinism lints: the common ambient clock and entropy sources, and unordered maps

**Status:** accepted (foundation CI/lab). **IDs affected:** CON-5, TRC-24.

## Context
CON-5(b) names `Instant::now()`, `SystemTime::now()`, `rand::thread_rng()` "and equivalents". The shipped lint list had only those three plus `tokio::time::Instant::now` and `thread::sleep`; CLAUDE.md claimed `fastrand` was banned when it was not, and `rand` 0.9 renamed `thread_rng` to `rng`. Separately, CON-5(c) and TRC-24 require byte-identical bundles, and the most common way Rust programs lose that property is iterating a `HashMap`, whose order is randomised per process.

## Decision
- `disallowed-methods` lists the common equivalents explicitly (a lint list can never be complete; the byte-identity acceptance test TRC-24 is the backstop): `rand::rng`, `rand::random`, `fastrand::Rng::new`, `getrandom::fill`, `uuid::Uuid::new_v4`, `chrono::{Utc,Local}::now`, `time::OffsetDateTime::now_utc`, `RandomState::new`, the `fastrand` free functions, `getrandom::getrandom`, `SystemTime::elapsed` and `ahash::RandomState::new`. Paths that only exist in some versions of a crate carry `allow-invalid = true`.
- `disallowed-types` bans `std::collections::HashMap` and `HashSet`, their `hashbrown` and `ahash` equivalents, and `rand::rngs::{OsRng, ThreadRng}` across the workspace, with `disallowed_types = "deny"`. Substrate code uses `BTreeMap`/`BTreeSet`; a crate that needs hashing for speed adds an insertion-ordered or fixed-hasher map through an ADR and a local `#[allow]` with the reason.
- `acn_emu::clock` and `acn_emu::rng` are the only modules expected to carry `#[allow(clippy::disallowed_methods)]`, per CON-5(b).

## Consequences
The ban is coarse on purpose: it is cheaper to justify one exception than to audit every map for whether its order leaks into a bundle. Lints see only first-party code, so a dependency that iterates a `HashMap` into output is caught by the byte-identity acceptance test (TRC-24), not here.

## Known limits
Clippy's `disallowed_types` fires on a type in type position, not on a unit struct used as a value, so `let mut r = rand::rngs::OsRng;` passes; and a method list can never enumerate a crate (`fastrand` has dozens of free functions). The list is a tripwire for the common mistakes, not a proof. The proof is the byte-identity acceptance test (TRC-24) and review of any new dependency that provides entropy or time: `cargo deny` output shows when `rand`, `fastrand` or `getrandom` first enters the substrate's graph.

## Amendment (pre-landing review) — the configuration cannot be shadowed, and two lists became bans
- **Shadowing.** Clippy reads the nearest `clippy.toml` above a crate and prefers `.clippy.toml`; `CLIPPY_CONF_DIR` or `--cap-lints allow` in `.cargo/config.toml` replace or silence the lot. Each was shown to pass every gate. Tests now require that the only such files are `/clippy.toml` and `/lab/clippy.toml`, that `.cargo/config.toml` is exactly the `xtask` alias (an alias named `deny` replaces `cargo deny`), that `[workspace.lints]` is exactly the agreed table, and that no source file outside `acn_emu::clock` and `acn_emu::rng` mentions a `clippy::disallowed_*` lint, since `deny` yields to a local `#[allow]`.
- **`RandomState`** is a disallowed type, so `RandomState::default()` and `BuildHasher` use are caught as well as `::new`. **`Instant::elapsed`** and `getrandom::{u32,u64}` join the method list.
- **`fastrand`** is banned in `deny.toml` with `tempfile` as the only permitted parent. A method list could not cover its free functions; a dependency ban can. The lists in `clippy.toml` and `deny.toml` are compared whole by the tests, so an entry cannot be dropped or given a `wrappers` exemption unnoticed.
- **Still open.** Paths for crates not yet in the graph (`tokio`, `rand`, `chrono`, `time`, `uuid`, `ahash`) cannot be checked until the crate arrives; clippy reports an unresolvable path as a warning about the configuration, not as a lint failure. `cargo deny` checks the default-feature graph, while clippy builds all features, so an optional dependency is compiled without a source check; widening the chain is a CON-9 change. `std::process::id()`, file modification times and trait-method entropy (`SeedableRng::from_entropy`) are not listed. TRC-24 remains the proof.
