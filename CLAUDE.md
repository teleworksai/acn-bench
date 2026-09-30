# CLAUDE.md — acn-bench development agent contract

You are implementing **acn-bench**, a Rust workspace that builds the ACN experimental substrate and runs the POC catalogue (see `PLAN.md`). ACN is an exploration of a new field, so the repo has two tracks with different rules. **`lab/` is the exploration track**: try anything, in Rust, with only fmt + clippy + a lab note as gates (CON-23); nothing there needs a spec, an ID, a control or a hypothesis file. **`crates/`, `specs/`, `hypotheses/` are the substrate track**, where results become defensible and the rules below apply in full. An idea graduates from lab to substrate when someone wants to cite a number from it (CON-24). If you are unsure which track a task is on, it is lab. Read, in this order, before any task: `specs/000-constitution.md` (invariants; RFC 2119 language), the target `specs/<NNN>-*.md`, then the task entry in `TASKS.md`.

## The loop (every substrate task)

1. **Restate the requirement IDs** you will satisfy (e.g. `EMU-3, EMU-4, TRC-2`).
2. **Write the tests first.** Unit tests in the crate, integration tests under `tests/`, acceptance tests under `tests/accept/`. Each test function carries a doc comment `/// Cites: EMU-3, EMU-4`. `cargo xtask trace-check` fails CI if an implemented MUST has no citing test (CON-12).
3. **Implement until the tests pass.** No `unsafe` (CON-19). Library code returns `Result`; no `unwrap`/`expect`/`panic!` outside tests and `main` (clippy enforces).
4. **Run all gates** (below), then open a PR whose description lists the requirement IDs. One spec concern per PR.

## Quality gates — run before every commit, chained with `&&`

```
cargo fmt --all --check \
&& cargo clippy --workspace --all-targets --all-features -- -D warnings \
&& cargo test --workspace \
&& cargo xtask trace-check \
&& cargo xtask docs-inventory --check \
&& cargo xtask env-hash --check \
&& cargo deny check
```

`tools/ci.sh` runs exactly this. Class B changes add the affected acceptance suites (`cargo test -p acn-accept --test <poc>`); `netem` changes add `cargo test --features netem -- --ignored` on Linux. Documentation-only changes may run `fmt`, `docs-inventory` and `trace-check` only.

## Non-negotiable rules

- **Specs are read-only.** You MUST NOT edit `specs/` except in a PR labelled `spec-change` with a rationale (CON-14). If a spec and a test disagree, STOP and open an issue `spec-conflict: <ids>` (CON-13).
- **Frozen hypotheses are frozen.** You MUST NOT edit `hypotheses/*.toml` or anything in the frozen set (CON-7). Candidate hypotheses in `lab/hypotheses/` are yours to edit and iterate on; freezing one is a Class C PR (CON-17). A negative result on a frozen hypothesis is reported, not re-hypothesised.
- **Sim has a live twin.** Any sim number you intend to cite runs in live on the same scenario; `acn hyp verdict` records the divergence in the verdict (CON-25). If they disagree, the simulator is wrong until shown otherwise.
- **Mock is not a result.** Bundles on `acn-mockllm` are labelled and never cited (CON-26). Provider-behaviour hypotheses (caching, streaming) are instantiated per provider.
- **The loop is layered (SPEC 085).** L0 build → L1 sim → L2 twin → L3 reality → L4 human. Use `acn loop run/twin/promote`; a layer never consumes unverified output from the layer below, and results at L1–L3 never edit frozen hypotheses or specs. Cite only `docs/evidence/` pages.
- **Every experiment has a control.** A POC acceptance suite that lacks the non-agent/plain-RPC control named in its spec is incomplete (CON-18).
- **Determinism is a feature.** Inject `Clock` and `Rng`; never call `Instant::now()`, `SystemTime::now()`, `rand::thread_rng()` or `fastrand` outside `acn_emu::clock` and `acn_emu::rng` (CON-5; clippy `disallowed_methods`). In `sim` mode the same seed MUST produce a bit-identical bundle.
- **CLI contract.** Every `acn` subcommand prints exactly one JSON object to stdout, logs to stderr via `tracing`, and exits 0 iff `"ok": true` (CON-8).
- **Data contract.** Traces are Arrow/Parquet (CON-4). JSON is only for CLI results, manifests and control-plane messages. No ad-hoc CSV on the run path.
- **Rust only.** No Python, shell beyond `tools/*.sh`, or notebooks in the run or analysis path. Plots come from `acn-attrib` (plotters → SVG). Exceptions require an ADR (CON-2).
- **Independence.** The substrate and every frozen result run without `dora-rs`, `aisle`, Genesis or ROS as required dependencies; optional adapters are fine as feature-gated or lab crates (CON-20). Ideas from elsewhere are welcome and get re-specified here when they graduate.
- **Ambiguity.** Write `docs/decisions/ADR-<n>.md` (context, decision, consequences, IDs affected) and proceed; do not stall (CON-15).
- **Commits.** Conventional commits `type(scope): subject` (`feat`, `fix`, `test`, `spec`, `docs`, `refactor`, `chore`, `ci`, `perf`). The repo squash-merges: the PR title becomes the mainline commit subject. Long bodies via `--body-file`, never inline heredocs. Branches `feat/…`, `fix/…`, `docs/…`, `spec/…`.
- **Review and merging.** While `.github/CODEOWNERS` names one owner (check it; do not assume), the project is in solo-maintainer mode (CON-16): independent review is not a merge condition. The maintainer is the human owner, not you: never merge a PR on your own initiative, and run a merge only when the maintainer tells you to for that PR. When you are asked to review, for a Class C change, or for a Class B change the maintainer judges risky, run the matching prompt in `TASKS.md` (adversarial for Class C, cross-review otherwise) from a separate session, comment findings, and do not push. Once CODEOWNERS names a second owner, every PR needs an approving review from a person other than its author. After the M0 gate, Class C PRs get the adversarial review of CON-7, linked from the PR.

## Risk classes (CON-10)

- **Class A** — `docs/`, `tests/`, `tools/`, `xtask`, `scenarios/synthetic/`: baseline gates.
- **Class B** — run-path crates `acn-emu`, `acn-mockllm`, `acn-harness`, `acn-gen`, `acn-replay`, `acn-ctl`, `acn-cli`: baseline gates + affected acceptance suites.
- **Class C — frozen set** — `hypotheses/`, `scenarios/measured/`, `crates/acn-hyp`, `crates/acn-attrib/src/core/`, `crates/acn-trace/src/schema/`: human-merged PR labelled `env-change`, updated `env-hash`, adversarial review. Post-M0 nothing in the frozen set changes without this.

## Rust conventions

- Toolchain pinned in `rust-toolchain.toml`; edition 2024; MSRV = pinned stable. `#![forbid(unsafe_code)]` in every crate.
- Errors: `thiserror` in libraries, `anyhow` only in binaries. Tracing: `tracing` + `tracing-subscriber` (stderr, JSON when `ACN_LOG=json`).
- Async: tokio only; `#[tokio::test(start_paused = true)]` for anything time-dependent; no `std::thread::sleep` on the run path.
- Serialization: `serde` with `#[serde(deny_unknown_fields)]` on every config/scenario/hypothesis type; TOML for configs and hypotheses, Parquet for traces, JSON for control-plane and CLI.
- Hashes: `blake3`; run IDs and env hashes are hex-encoded blake3.
- No global mutable state; no `lazy_static` for anything seeded.
- Feature flags: `netem` (Linux only), `real-api` (network calls to real inference endpoints; off in CI).
- When unsure about a crate's API, read its source in `~/.cargo/registry` rather than guessing signatures.

## Test tiers (CON-12)

| Tier | Where | Runs |
|---|---|---|
| `unit` | `#[cfg(test)]` in crates | every `cargo test`, CI on every PR |
| `integ` | `tests/` in crates, `sim` mode, mock backends | every `cargo test`, CI on every PR |
| `accept` | `tests/accept/<poc>.rs`, cites POC spec IDs | CI when the POC's crates change; nightly all |
| `live` | `#[ignore]`, requires sockets/real API (`--features real-api`) | manual and nightly |
| `netem` | `#[ignore]`, Linux + root | nightly on Linux runner |

## Lab track (CON-23, CON-24)

- Put spikes, throwaway simulators, partner-stack wrappers and candidate hypotheses under `lab/<slug>/`. Create a crate with `cp -R lab/_template lab/<slug>`, never with `cargo new` (it edits the root manifest; ADR-7), then rename the package (steps in `lab/README.md`). Depend on whatever you need.
- Gates, per lab crate (lab/ is excluded from the workspace, so address the crate by manifest): `cargo fmt --manifest-path lab/<slug>/Cargo.toml --check && cargo clippy --manifest-path lab/<slug>/Cargo.toml --all-targets -- -D warnings`. `lab/clippy.toml` switches off the substrate's determinism bans. No trace-check, no citing, no control.
- Write `docs/lab/<yyyy-mm-dd>-<slug>.md`: question, what was tried, what was learned (numbers welcome, labelled exploratory), graduate / park / drop.
- To graduate: open a `spec-change` PR with the spec and IDs, promote the candidate hypothesis file, then follow the substrate loop. New POCs beyond the catalogue are expected to come from here.

## Definition of done for a substrate task

All gates green; every MUST in scope cited by a test; ADRs written for every interpretation you made; PR lists the IDs; and, once the project has a second maintainer, an approving review from someone other than the author (CON-16). For POC tasks: a bundle in `runs/` regenerated from its run ID, and a verdict from `acn hyp verdict` attached to the PR.
