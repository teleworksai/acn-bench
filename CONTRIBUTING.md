# Contributing to acn-bench

The working rules are in [`CLAUDE.md`](CLAUDE.md) and they apply to people and agents alike. The invariants are in [`specs/000-constitution.md`](specs/000-constitution.md). This page is the short version; where it and those two disagree, they win.

## Set up

```bash
git clone https://github.com/teleworksai/acn-bench && cd acn-bench
rustup toolchain install                              # the toolchain pinned in rust-toolchain.toml
cargo install cargo-deny --locked --version 0.20.2    # same version as CI
tools/ci.sh                                           # must exit 0 before you change anything
```

## Two tracks

- **`lab/`** — exploration. Any Rust, any dependency. Start with `cp -R lab/_template lab/<slug>`, never `cargo new`, then rename the package; the steps and the two gate commands are in [`lab/README.md`](lab/README.md). Write a lab note in `docs/lab/`.
- **`crates/`, `specs/`, `hypotheses/`** — the substrate, where numbers become citable. Everything below applies. A substrate crate never depends on a lab crate.

If you are not sure which track you are on, it is lab.

## A substrate change

1. Pick the task in [`TASKS.md`](TASKS.md). Restate the requirement IDs you will satisfy.
2. Write the tests first. Each test function carries `/// Cites: CON-5, TRC-24`.
3. Implement. No `unsafe`; no `unwrap`/`expect`/`panic!` in library code; inject `Clock` and `Rng`; no `HashMap` (ADR-8).
4. Add the IDs you implemented to `trace-scope.toml`. Record every interpretation as `docs/decisions/ADR-<n>.md`.
5. Regenerate `docs/generated/` with `cargo xtask docs-inventory`, then run `tools/ci.sh` (it checks that the generated docs are current).
6. Open a PR with a conventional-commit title and the template filled in. One spec concern per PR. `gh pr create --body-file` skips the template, so copy its headings into the body.
7. Review (CON-16). While CODEOWNERS names one owner, independent review is not a merge condition: the maintainer merges once the gates and the required CI checks are green. A review by a separate agent session should be requested for a Class C change (the adversarial prompt in `TASKS.md`) and for a Class B change the maintainer judges risky (the cross-review prompt). With a second maintainer, every PR needs an approving GitHub review from someone other than its author. An agent cross-review is PR comments in both modes and never counts as that approval. Agents do not merge on their own initiative.

## Labels

`cargo xtask pr-check` runs on every PR and fails when the PR

- touches `specs/`, or removes an entry from `trace-scope.toml`, without the **`spec-change`** label, or
- once the M0 gate is closed, touches the frozen set, `env-hash.json` or an existing record under `docs/gates/` without **`env-change`**. Before M0 this is reported as an advisory.

It also fails when `.github/CODEOWNERS` lacks an entry for a protected path or an enforcement point. A failing `pr-check` blocks the merge only while `pr-check` is a required status check on `main`; ADR-6 lists what the repository settings must be.

If a spec and a test disagree, stop and open a **spec-conflict** issue. Do not fix either in the same PR.

```bash
gh issue create --template "Spec conflict (CON-13)"
# or, without the template:
gh issue create --title "spec-conflict: <ids>" --label spec-conflict --body-file <file>
```

## Changing the gates themselves

Workflows, `clippy.toml`, `deny.toml`, `.cargo/config.toml`, `[workspace.lints]` and the root manifest are compared whole by `crates/xtask/tests/workspace.rs`. A change to one of them, such as a licence exception for a new dependency, is made twice: in the file and in the copy that test holds. CODEOWNERS must keep an entry for every path in `pr_check::ENFORCEMENT_POINTS`. Say in the PR that it touches an enforcement point.

## Useful commands

```bash
tools/ci.sh                          # the whole gate chain (CON-9); ordinary cargo output
cargo xtask trace-check              # every in-scope ID cited; ID references in the docs resolve
cargo xtask docs-inventory [--check] # docs/generated/
cargo xtask env-hash [--check|--write]
cargo xtask pr-check --base refs/remotes/origin/main --labels spec-change
```

Each `cargo xtask` command prints one JSON object on stdout and exits 0 only when `"ok": true` (CON-8).

## Licence of contributions

Contributions are accepted under the repository's licence, Apache-2.0, as its section 5 states.
