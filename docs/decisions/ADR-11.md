# ADR-11 — T02 lands as a series of PRs

**Status:** accepted (T02). **IDs affected:** CON-11; TRC-1 to TRC-38, CON-27 to CON-31 (as implemented by the series).

## Context
`TASKS.md` says each substrate task is one PR, and CON-11 says one spec concern per PR. T02 implements most of SPEC 010: the attribute inventory and provider mappings in the frozen schema module, identity hashing with its known-answer vectors, the span model and Parquet writer, seeded identifiers, the bundle with its manifest and verifier, the ingester and five derived views, OTLP export and import, report coverage and clock-offset correction. As one PR that is several thousand lines across a frozen directory, a run-path crate, the CLI and the acceptance suite: it could not be reviewed, and a Class C change would be buried in Class B code.

## Decision
T02 is delivered as a prerequisite and four PRs, each tests-first, each green on its own, merged in order:
1. **Prerequisite.** `cargo xtask env-hash` computes and records `engine_hash` (CON-28), because every manifest carries it.
2. **T02a, Class C.** The frozen schema module: `SEMCONV_VERSION`, `acn_attributes.toml` with types, units, producers, promoted flags, option defaults and the per-provider mappings, `views.toml`, their strict loaders, and the generated attribute page (TRC-2, TRC-3, TRC-20, TRC-21, TRC-37).
3. **T02b, Class B.** Identity: `run_id`, `params_hash`, derived seeds and sub-stream seeds with known-answer vectors (CON-27, CON-29, CON-30). The span model, the OTLP-shaped Parquet writer with fixed settings, the seeded `IdGenerator`, the bundle, the manifest, `bundle_digest`, and `acn bundle verify` (TRC-22 to TRC-27).
4. **T02c, Class B.** The ingester and the session, turn, call, link and tool views on a golden fixture (TRC-30 to TRC-35, TRC-38).
5. **T02d, Class B.** OTLP export and import, report coverage, and clock-offset correction (TRC-26, TRC-28, TRC-36).

IDs enter `trace-scope.toml` in the PR that implements them. Build identity (CON-31) needs a build script in `acn-cli` and lands with T02b if it stays small, otherwise as its own PR before T03.

## Consequences
T02 is complete when the fifth PR merges. `TASKS.md` keeps T02 as one entry; this ADR is the map. The same slicing rule applies to later tasks of similar size: split along risk class and along what can be green alone.
