# ADR-21 — The readings T05.3 makes: read-only files, no relaxing, the freeze PR

**Status:** accepted (T05.3, Class C). **IDs affected:** HYP-1, HYP-4, HYP-9, HYP-23, HYP-25, HYP-26, HYP-27; CON-7, CON-17, LOOP-13.

## Context
T05.3 closes SPEC 080 with three process rules:
- hypothesis files are read-only to the tooling, and a file that changes mid-use aborts it (HYP-4);
- no option relaxes a hypothesis, and a `fail` is a result (HYP-25);
- the freeze PR has a fixed shape (HYP-26).

Each needs some reading before a machine can check it. The loop runner (SPEC 085) does not exist yet.

## Decision

### HYP-4: read-only, and nothing written outside `runs/`
- **One writer, checked structurally.** `xtask`'s `workspace.rs` parses every source file of `acn-hyp` with `syn`. It finds every path that names a call able to create, change or remove a file:
  - `fs::write`, `copy`, `create_dir`, `remove_*`, `rename`, `hard_link`, `soft_link`, `set_permissions`;
  - `File::create`, `create_new` and `options`;
  - `OpenOptions`, `symlink` and `Command`;
  - a `use std::fs::{write}` that brings one in under its bare name.

  It requires every one to sit inside `verdict::write`.
  - A crate-level `clippy.toml` ban was the first try. ADR-8 rejects it: a nearer `clippy.toml` shadows the workspace's CON-5 bans, and only `acn_emu`'s clock and RNG modules may switch them off.
  - The loop runner, which also writes under `runs/`, will widen the allowance in its own Class C PR.
- **Where verdicts may go.** `write` accepts a directory only when:
  - it is named `runs`;
  - it is a real directory, not a symbolic link;
  - when a workspace root (CON-28) holds it, it is that root's own `runs/`. This refuses `hypotheses/runs`, `scenarios/runs` and every other directory named `runs` inside a workspace.

  Each level (`runs/`, `verdicts/`, the verdict's directory) is created and checked not to be a symbolic link before anything is created beneath it. Outside any workspace, a directory named `runs` is accepted, because there is no frozen set to protect.
- **`hypothesis_changed`.** `Hypothesis::check_unchanged` re-reads the file and returns the typed error `HypothesisChanged`, which renders as `hypothesis_changed: …`. It carries the hash the file was loaded with and its hash now, if it can still be read.
  - `verdict::judge_and_write` judges, re-checks the file, then writes. An edited file aborts the write with `VerdictError::HypothesisChanged`, and nothing is written.
  - `acn hyp verdict` uses `judge_and_write`.
  - SPEC 085's loop calls `check_unchanged` before every step (LOOP-13). That half of HYP-4 is the loop's to test.
- **Tests.** `readonly.rs` runs the whole use of a file twice: load, lint, judge and write.
  - With `hypotheses/` writable, as SPEC 080 §5 names it, a snapshot of every file and directory outside `runs/` is unchanged.
  - With it made read-only, the file is still read. Permissions are restored by a drop guard.
- **CI.** `xtask`'s `workspace.rs` parses both workflows. Every permission granted, at the top level or by a job, must be `read` or `none`, and `contents` must be `read`. Agent and account write access remain ADR-6's statement of intent (SPEC 080 §6, question 10).

### HYP-25: nothing relaxes a hypothesis
- **No mutation in memory.** A `Hypothesis` is read through accessors only; its fields are private to `acn-hyp`.
  - No caller can change a loaded file's design, guard, predicate, parameters or expected outcome before judging with it.
  - That includes changing `replicates` to turn a `fail` into `inconclusive` under the same `verdict_id`, which the review showed was possible while the fields were public.
- **The verdict's inputs.** `verdict` takes the hypothesis, the bundles and the engine hash, and nothing else.
- **The CLI.** A unit test in `acn-cli` walks the `acn hyp` command through clap's `CommandFactory`, hidden subcommands and arguments included.
  - It pins the two subcommands, `lint` and `verdict`.
  - It pins their exact arguments, short, long and positional: the file; and the hypothesis, the bundles and `--runs-dir`.
  - clap's `env` feature is off, so no argument reads the environment.
- **A `fail` is written like any verdict.** It has the same layout as a `pass`, is never refused, and its status is `frozen`. Recording it on the evidence page is LOOP-3's, in SPEC 085.

### HYP-9 reading: `[design].backends`
`backends` names the ways of providing inference a hypothesis is about.
- A file that names none is about the mock only.
- A verdict refuses any bundle whose backend the file does not name: `mockllm` for the mock, `real-api` for any real provider.
- So a file cannot leave out `real-api` to escape the pins HYP-26 asks for, or the `unpinned-inputs` label of HYP-23.

### HYP-26: the freeze check and the freeze PR's shape
- **The freeze check** is that `env-hash.json` records the frozen set as it stands. `cargo xtask env-hash --check` compares the whole record, and a changed hypothesis file then loads as a candidate (HYP-3). `crates/acn-hyp/tests/freeze.rs` tests this through `acn_trace::env`.
- **Only `.toml` files.** A hypothesis file is `<id>.toml` (HYP-1): the loader refuses any other name, so nothing else under `hypotheses/` can load as frozen. CI's lint run (`tests/accept/hyp_lint.rs`) walks every file under `hypotheses/`, subdirectories included, whatever its name.
- **`pr-check` checks every file the PR adds or changes under `hypotheses/`.** The file must:
  - be named `*.toml`;
  - load (HYP-1), which already enforces the named POC spec and a consistent `[poc].status` (HYP-2, HYP-3);
  - load as frozen, so this PR's `env-hash.json` must record it;
  - lint clean (HYP-27);
  - carry `[design].pins` when `real-api` is among its backends.
- **In `--base` mode** (CI), a working tree that differs from the head commit, for the file or for `env-hash.json`, is itself a finding. What is judged is what would merge.
- **Labels.** These findings carry no label (`label: null`), because no label satisfies them.
- **The `env-change` label and the adversarial review** are CON-7's. `pr-check` enforces them after M0 and advises before it.
- **Subdirectories** under `hypotheses/` are allowed.
- **A removed file** is CON-7's frozen-set change, not a freeze.
- **The pins rule binds on change.** `hypotheses/p4.toml` has no pins today (HYP-16, §6 question 4), so its first change must add them.
- **Where the tests live.** The `pr-check` half is tested in `crates/xtask/tests/freeze.rs`, in list mode and in git `--base` mode with renames and uncommitted edits. `acn-hyp` therefore does not depend on `xtask`, so no dependency cycle ties a Class A tool's API to a frozen test.
- **Who checks the checker.** `pr-check` is built from the PR's head, so a PR that edits both `hypotheses/` and `acn-hyp`'s lint judges itself. That is the self-judging gap ADR-6 records for every enforcement point.
- **Not checkable.** That the freeze precedes the first cited run (CON-17), and that a narrowing carries `supersedes`, remain review items.

## Consequences
SPEC 080 is implemented, except the halves that belong to SPEC 085:
- the loop's use of `check_unchanged` (HYP-4);
- the evidence page (HYP-25).

`trace-scope.toml` says so, and those SPEC 085 tests must cite HYP-4 and HYP-25 too.
