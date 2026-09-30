# ADR-7 — `lab/` is outside the Cargo workspace and outside the determinism lints

**Status:** accepted (foundation CI/lab). **IDs affected:** CON-2, CON-6, CON-23.

## Context
CON-23 lets lab crates depend on anything and exempts them from CON-4..18. As shipped, the root manifest neither listed nor excluded `lab/`, so cargo rejected every crate created there ("current package believes it's in a workspace when it's not"), and clippy, which uses the nearest `clippy.toml` above a crate, applied the substrate's CON-5 bans to lab code. The first lab spike (a QUIC timing prototype) needs `Instant::now()`.

## Decision
- The root manifest carries `exclude = ["lab"]`, and every lab crate carries an empty `[workspace]` table, which makes it its own workspace root. The second part is what actually isolates it: `cargo new lab/<slug>` ignores a prefix `exclude`, adds the crate to the root `members` and writes `workspace = true` inheritance, so lab crates are created by copying `lab/_template`, never with `cargo new`. CON-2's "one Cargo workspace" is read as applying to the substrate; lab roots are outside it by CON-23. Each lab crate is standalone: its own `Cargo.lock`, its own `target/`, any dependency it likes, none of it in the substrate's lockfile or `cargo deny` graph.
- `lab/clippy.toml` exists and carries no `disallowed-*` entries, shielding lab crates from the root configuration.
- Lab gates address the crate by manifest path (`cargo fmt|clippy --manifest-path lab/<slug>/Cargo.toml`). `target/` is ignored at any depth.
- This is not a substrate layout change under CON-6: no `crates/` member is added or moved.

## Consequences
A lab crate cannot use `workspace = true` inheritance; it states its own edition and dependency versions, which also keeps exploratory dependencies out of the reproducibility story. Graduation (CON-24) moves code into `crates/` under a `spec-change` PR, at which point the full lint posture applies.

## Amendment (pre-landing review) — the boundary holds in both directions
`exclude = ["lab"]` keeps lab crates out of the workspace, but nothing stopped a substrate crate from naming one as a path dependency. That compiles lab code into the substrate with no lint (clippy checks workspace members only, and `lab/clippy.toml` shields the path), puts its dependencies in the substrate lockfile, and shows no source to `cargo deny`. A test now requires that every package in `Cargo.lock` without a `source` is a workspace member, which also catches a vendored copy and a `[patch]` to a path; the root manifest may contain `[workspace]` only. The template carries `#![forbid(unsafe_code)]`, because CON-23 lifts CON-4 to CON-18 and CON-19 is not among them. The test for the empty `[workspace]` table copies the template into an unrelated workspace, so it proves the table and not the root `exclude`.
