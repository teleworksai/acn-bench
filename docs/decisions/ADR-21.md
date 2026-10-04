# ADR-21 — The readings T05.3 makes: read-only files, no relaxing, the freeze PR

**Status:** accepted (T05.3, Class C). **IDs affected:** HYP-4, HYP-25, HYP-26; CON-7, CON-17, LOOP-13.

## Context
T05.3 closes SPEC 080 with three process rules:
- hypothesis files are read-only to the tooling, and a file that changes mid-use aborts it (HYP-4);
- no option relaxes a hypothesis, and a `fail` is a result (HYP-25);
- the freeze PR has a fixed shape (HYP-26).

Each needs some reading before it can be checked by a machine. The loop runner (SPEC 085) does not exist yet.

## Decision

### HYP-4
- **Read-only access.** `acn-hyp` opens hypothesis files only to read them: `fs::read`, `read_to_string`, `read_dir` and `canonicalize`.
  - The only code in the crate that creates, changes or removes a file is `verdict::write`. A test scans the crate's source for every write-capable call to keep it that way.
  - Another test makes `hypotheses/` and its file read-only, then loads, lints, judges and writes a verdict. It checks that nothing outside `runs/` changed.
- **Writing only under `runs/`.** `verdict::write` refuses a directory that is not named `runs` (ADR-20), and `acn hyp verdict` uses it.
- **`hypothesis_changed`.** `Hypothesis::check_unchanged` re-reads the file and fails with the error `hypothesis_changed` when its bytes no longer have the hash it was loaded with, or it cannot be read.
  - `acn hyp verdict` calls it before writing, so a verdict always belongs to the bytes it was judged against.
  - SPEC 085's loop calls it before every step (LOOP-13). That half of HYP-4 is the loop's to test.
- **Write access for agents and CI.** The tooling cannot enforce this for agents. For CI, a test requires every workflow to declare `contents: read` and no write permission. Agent and account permissions remain ADR-6's statement of intent (SPEC 080 §6, question 10).

### HYP-25
- **No relaxing option.** `acn hyp` has two subcommands, `lint` and `verdict`.
  - `verdict` takes only `--hypothesis`, the bundles and `--runs-dir`; a CLI test pins that list.
  - A source test rejects words such as `relax`, `override`, `force`, `skip`, `exclude`, `ignore`, `tolerance` or `rescope` in the declaration of `acn hyp`.
  - The library's `verdict` takes the file, the bundles and the engine hash, and nothing else.
- **A `fail` is written like any verdict.** It has the same layout as a `pass`, is never refused, and its status is `frozen`. Recording it on the evidence page is LOOP-3's, in SPEC 085.

### HYP-26
- **The freeze check.** `cargo xtask env-hash --check` fails on a changed hypothesis file, and that file then loads as a candidate (HYP-3).
- **The shape of a freeze PR.** `cargo xtask pr-check` now checks every file the PR adds or changes under `hypotheses/`. The file must:
  - load (HYP-1), which already enforces the named POC spec and a consistent `[poc].status` (HYP-2, HYP-3);
  - load as frozen, so this PR's `env-hash.json` must record it;
  - lint clean (HYP-27);
  - carry `[design].pins` when `real-api` is among its backends.
- **Labels.** The `env-change` label and the adversarial review are CON-7's, which pr-check already enforces.
- **The pins rule binds on change.** `hypotheses/p4.toml` has no pins today (HYP-16, §6 question 4). Its first change must add them.
- **`freeze.rs` and `xtask`.** `freeze.rs` drives `xtask`'s `env-hash` and `pr-check` through a dev-dependency, so the test exercises the gates themselves.
- **Not checkable.** That the freeze precedes the first cited run (CON-17), and that a narrowing carries `supersedes`, are not visible to either check. They remain review items.

## Consequences
SPEC 080 is implemented in full, except for the halves that belong to SPEC 085: the loop's use of `check_unchanged` (HYP-4) and the evidence page (HYP-25).
