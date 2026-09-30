# ADR-5 — T01 scaffolds every CON-6 crate as a stub; CLI argument errors stay inside the JSON contract

**Status:** accepted (T01). **IDs affected:** CON-6, CON-8, CON-19.

## Context
The T01 kickoff says "do not add acn-emu, tokio proxies or any network code yet", while CON-6 fixes the crate list and the workspace manifest already names all eleven crates plus `tests/accept`; cargo cannot resolve a workspace whose members do not exist. Separately, CON-8 says every `acn` and `cargo xtask` entry point prints exactly one JSON object and exits 0 iff `ok`, which clap's default handling of bad arguments and `--help` does not satisfy.

## Decision
- Every CON-6 crate exists after T01 as a dependency-free stub: manifest inheriting the workspace package fields and lints, a documented `lib.rs` with `#![forbid(unsafe_code)]`, and the frozen-set module directories (`acn-trace/src/schema/`, `acn-attrib/src/core/`) so `env-hash` has stable roots from day one. `acn-emu` is a stub with no clock, RNG or proxy code; its content is T10–T12. `tests/accept` is the `acn-accept` package with `autotests = false` so each POC suite is declared as a `[[test]]` at `tests/accept/<poc>.rs`, matching the file names the specs use.
- Both binaries parse with `try_parse`. An argument error becomes `{"ok": false, "error": …}` with exit 1. `--help` and `--version` print their text to stderr and emit `{"ok": true}` so the stdout contract holds on every path.
- CON-19's exemption for tests is realised two ways: clippy's `allow-unwrap-in-tests`, `allow-expect-in-tests` and `allow-panic-in-tests` cover `#[test]` bodies and `#[cfg(test)]` modules, and integration-test files carry one crate-level `#![allow(...)]` for their helper functions, which clippy does not recognise as test code. Library targets stay under `deny`.

## Consequences
The workspace builds and all gates run from the first PR. Later spec tasks replace stub contents; no member list changes are needed until a spec-change adds a crate. Help output on stderr is unusual but keeps stdout machine-parseable, which is the property CON-8 exists to guarantee.
