# ADR-41 — T16: POC 16 as regeneration records, and where evidence pages come from

**Status:** accepted (T16.1, spec-change). **IDs affected:** P16-1 to P16-30, LOOP-30, CON-31.

## Context
T16 delivers M1's "a second machine regenerates the bundle" and the kit (PLAN.md, M1).
- Nothing could regenerate a bundle from its `run_id`. A manifest records hashes, not paths, and only loop reports (LOOP-14) and control-plane requests (CTL-11) record paths.
- CON-31 claims byte identity only within one build, and `build_hash` includes the target and the source tree.
- TASKS.md asked for `hypotheses/p16.toml`, but a frozen hypothesis needs a falsifier over verdict quantities, and "regenerates byte for byte" is not one.
- LOOP-30's evidence pages read loop reports, which live in the gitignored `runs/`, so CI's `docs-inventory --check` could never build them.

## Decision
The maintainer chose each of the following (October 2026).
- **Reproduction is same-target, with cross-target evidence.**
  - A second machine on the same tag and target, building with `--locked`, gets the same `build_hash`, and must regenerate every reference manifest of its target byte for byte (P16-11).
  - CI regenerates within one build on each of its three targets, and compares the targets as a published artifact, never as a failure (P16-12).
  - CON-31 is unchanged.
- **POC 16 is records, not a verdict.** There is no `hypotheses/p16.toml` (P16-1). Its evidence is the second-machine records under `docs/runs/` and the CI comparison, which the M1 gate checks (T17).
- **Evidence pages render from committed reports.** A cited loop's `report.json` and final `verdict.json` are committed under `docs/runs/<loop_id>/`, and `docs-inventory` renders `docs/evidence/` from them and the gate documents (P16-20, P16-21). Bundles stay in `runs/`.
- **Four PRs** (ADR-11):
  - T16.1: this spec.
  - T16.2: `acn run --from-run-id` and `tests/accept/kit.rs`.
  - T16.3: CI's per-target regeneration and comparison.
  - T16.4: evidence pages and the README quickstart.

  All are Class A or B: input finding lives in `acn-cli`, and `acn evidence verify` is unchanged.

Interpretations made in the spec:
- **Inputs are found by hash under fixed search roots** (P16-3): `workloads/`, `scenarios/`, `lab/`, `kit/inputs/` and the control plane's scenario store. Hypotheses are found under `hypotheses/` and `lab/hypotheses/`, and the status of the one found must be the manifest's, because the status is part of `run_id`.
- **A bundle's scenario is taken from the bundle itself,** whose `acn.scenario` span records the scenario's full text.
- **The run kind is read from `producers`,** because the manifest has no kind field.
- **`ok` follows LOOP-14 on the same build.** A regeneration that differs is `ok: false`. Across builds, `ok` reports completion and `identical` is the evidence. This matches LOOP-14's and ADR-22's `not_regenerable_with_this_build`.
- **The quickstart is executable** (P16-30): one fenced `sh quickstart` block, run by a test, so the README cannot drift from the binary.

## Consequences
- The reference manifests in `kit/manifests/` are valid for builds with their `build_hash`. A commit changes it only when it touches `crates/`, `Cargo.toml`, `Cargo.lock`, `rust-toolchain.toml` or `.cargo/config.toml`, so the manifests can be committed in the tagged commit itself. A release that changes any of those regenerates them.
- No frozen file changes in T16, and `engine_hash` does not move.

## After review (T16.1)
One read-only review found 10 findings, 3 blocking. SPEC 140 was revised before its PR:
- **Verify unchanged.** `acn evidence verify` lives in frozen `acn-hyp` and reads only `runs/loop/` (LOOP-2). The spec no longer asks it to read committed copies. Input finding is in `acn-cli`, so nothing in the frozen set changes.
- **All layers committed.** Evidence pages need the twins and the L2 and L3 verdicts as well as the report and the final verdict. All are committed, under `docs/runs/loop/<loop_id>/` and `docs/runs/verdicts/<verdict_id>/`. `--check` fails when one that a committed report names is missing.
- **Build-neutral comparison.** Every resource row records `acn.build_hash` (TRC-19), so a comparison of raw files across builds could never be identical. It now compares a build-neutral form:
  - `resources.parquet` decoded, without `acn.build_hash`;
  - the manifest without `build` or `resources.parquet`'s hash.
- **Scenarios found by hash.** Scenarios are always found by hash on disk, so that a trace-driven scenario's trace resolves beside its file. The span's text is a cross-check.
- **Conditions for an equal `build_hash`.** These are stated: a clean tree under `crates/`, no user rustflags, and the dev profile. A second-machine record lists every `BuildInfo` component.
- **Hypothesis search.** It includes `kit/inputs/`, with status derived from location, as the harness derives it. `.gitattributes` gains `kit/**`, `lab/**/*.toml` and `docs/runs/**/*.json`.
- **The quickstart has no shell** (CON-2). `$RUN_ID` is read from the latest JSON that carries one, and the block's lines start with the build command or `acn`.
- **Search limits.** Only `*.toml` files are searched, `target/` is skipped, and `<runs>` is resolved against the root.
- **More tests.** §6 now covers every refusal, the duplicates rule, a trace-driven scenario, and the committed files. P16-11 is checked by the M1 gate, not by a test.
- **Smaller points.** An unknown producer set is refused. A generator's backend and model are checked rather than set. The regeneration's bundle lands at `regen/<run_id>/<n>/<run_id>/`. Refusals carry a `code`. `--across-builds` reaches builds of one engine only.

## T16.2 notes
- **Where the code lives.** `acn_cli::regen` holds the regeneration. `acn run --from-run-id` calls it with the binary's own engine and build. The library takes them as arguments, so the acceptance suite can name another build or engine.
- **The base directory.** It is the workspace root when there is one (CON-28), and the start directory otherwise, as for a run with no hypothesis or a candidate one. `--runs-dir` resolves against it.
- **The inputs.** They are found by hashing every `*.toml` file under the search roots on each regeneration. No index is kept, so nothing can go stale. The files are few, and directories named `target` are skipped.
  - A hypothesis's status is matched by location: frozen exactly under `<base>/hypotheses/`, as the harness derives it.
  - With no bundle, the scenario is found by hash like any other input. With the bundle, the regenerated run records the file's bytes in its own scenario span. Because `acn.scenario.toml` is in the trace, the same-build comparison covers it.
- **The recorded options.** The harness writes `opt.` values as floats (CON-27(c)), and they are read back as whole numbers where `Opts` holds integers. An `opt.` that no run can set (`keep_content`) is `manifest_invalid`.
- **Refusal codes.** There are two beyond P16-6's list, both from the spec's words:
  - `unknown_run`, for a `run_id` with neither a bundle nor a reference manifest;
  - `run_failed`, for a run that fails.
- **What the suite shows across builds.** The build-neutral form already agrees: a harness bundle regenerated by a build with another `source_hash` is identical once the build is taken out. P16-12's cross-target records will show whether that also holds across targets.
- **After review (T16.2).** One read-only review found no way to get `identical: true` from differing bundles, and no write outside `runs/regen/`. Its findings, and what was done:
  - **A negative test.** A valid bundle that differs (a table swapped, its hash rewritten, the manifest kept canonical) must be `identical: false`: on the same build (`ok: false`, `differ` naming the file and `manifest.json`), across builds, and from a reference manifest alone.
  - **Bytewise order.** Candidates are now sorted by their path's bytes, as P16-3 says, not component by component. A unit test pins the case where the two orders disagree (`x-y/` before `x/`), and that only `*.toml` files are inputs.
  - **One walk.** The search roots are walked once per regeneration into a list of (path, hash) pairs.
  - **A generator's backend and model** are checked against its sheet before anything runs (P16-4), rather than surfacing later as `run_id_differs`.
  - **Fresh directories.** `fresh` takes `n` by `create_dir`, so a directory made concurrently is never shared. A failed regeneration keeps the `n` it took.
  - **The `run_id` argument** must be 64 lowercase hex before it is joined into any path (`unknown_run`).
  - **The stall threshold** read from a manifest must be finite and non-negative.
  - **The scenario span.** P16-3's check that the span's text is the file found needs no code. The file found has `scenario_hash`, and so does the original's span text (EMU-37). On the same build the spans are compared byte for byte, so a difference shows in `differ`, not as a refusal.
  - **First by path is followed literally.** A copy of a trace-driven scenario without its trace, placed earlier in path order (under `lab/`, say), is used, and the run fails. The kit keeps a scenario and its trace together under `scenarios/` (ADR-33).
  - **Left as is.** `Opts::default()` repeats the defaults that `acn_attributes.toml` declares; a drift would show as `run_id_differs`. No test resolves `--runs-dir` against a workspace root from a subdirectory, because a root needs the engine its binary embeds; the CLI test runs with no root.
