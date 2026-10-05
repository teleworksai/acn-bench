# ADR-23 — T05b.1: the L1 loop runner, and the readings SPEC 085 leaves open

**Status:** accepted (T05b.1, Class C: `crates/acn-hyp`). **IDs affected:** LOOP-1, LOOP-3, LOOP-10, LOOP-11, LOOP-13, LOOP-14, LOOP-15; HYP-4, HYP-9, HYP-20; CON-8, CON-27, CON-31.

## Context
SPEC 085 v0.2 and ADR-22 define the L1 loop. T05b is too large for one review, so it lands in two parts, as T05 did (ADR-18):
- **T05b.1 (this one):**
  - `acn loop run` with `grid` and `random`;
  - the loop report, `--from-report`;
  - the write direction (LOOP-3, 10, 11, 13, 14, 15).
- **T05b.2:**
  - `acn evidence verify` (LOOP-2);
  - the LOOP-4 gates;
  - the verify half of LOOP-1, which needs the chain walk.

  The layer derivation (`acn_hyp::layer`) lands now, and `loop_run.rs` cites LOOP-1 ahead of scope.

Implementing the runner needs these readings.

## Decision

### Where things live (LOOP-15)
- **Decisions in `acn_hyp::loop_run`.** The executor trait `Executor` runs one `Request` (exactly HAR-50's inputs) and answers two questions the frozen crate cannot answer without the run path:
  - whether a model is a mock profile (`check_model`);
  - whether a file loads as a workload (`check_workload`).

  Both are facts. The refusal they lead to is decided in `acn-hyp`.
- **The executor.** `acn-cli`'s `loop_exec::HarnessExecutor` calls `acn_harness::run::run` in `sim` on the mock, with the embedded profiles and `Opts::default()`. `acn-cli` therefore now depends on `acn-mockllm` at build time, not only in its tests.
- **The tests.** `acn-hyp`'s tests use an executor of their own over the real harness, so `acn-hyp` gains `acn-harness` and `acn-mockllm` as **dev**-dependencies. The library still does not depend on the harness. That executor's profiles cache the smoke workload's short prompts, as the T05.2b acceptance suite's do.
- **Writes.** All of them are in `acn_hyp::loop_out`, beside `verdict::write`.
  - `loop_out` writes the report and creates the regeneration directory, under a `runs/` that `verdict::check_runs_dir` accepts.
  - Every level is created and then checked not to be a symbolic link.
  - **The report is written all or nothing.** `report.json` and `report.md` go into a staging directory, `runs/loop/.<loop_id>.partial.<k>/`, with `k` the first index this writer can create. That directory is then renamed to `<loop_id>/`. A crash leaves at most a staging directory, which blocks nothing, so a loop can never be left with a report and no rendering. An existing `<loop_id>/` is never replaced: it is checked before staging and again just before the rename.
  - **Temporary names are unique per writer**, here and in `verdict::write` (`.verdict.json.partial.<k>`), so two concurrent writers never share or delete each other's file.
  - xtask's `workspace.rs` writer scan now allows `loop_out.rs`, as ADR-21 said the loop runner's own Class C PR would.

### Paths and the base (LOOP-10, LOOP-11)
- **The base.** Recorded paths are relative to the directory `runs/` lies in. Inside a workspace this is the root (CON-28), because `check_runs_dir` accepts only the root's own `runs/`. Outside one it is wherever `runs/` is.
- **Resolution.** `acn loop run` resolves `--hypothesis`, `--runs-dir` and `--from-report` against the workspace root found from the current directory, or against the current directory outside a workspace (LOOP-10). The runner resolves relative workload paths against the base.
- **Where the harness looks for the root.** Each `Request` carries `start_dir`, the base. The harness therefore decides a file's status (frozen or candidate) from the same root the loop did, wherever the process runs from. Otherwise a regeneration started outside the workspace would compute another run_id.
- **Paths outside the base** are refused with `path_refused`: a report could not record them relative to the base, so it could not regenerate.

### Running a loop (LOOP-10)
- **The run_id the runner expects** is computed in `acn-hyp` with CON-29's function: no `opt.` pairs, `scenario_hash` zero, mode `sim`, backend `mockllm`.
- **Checks on every returned bundle**, in this order:
  1. it verifies with its views (`read::read`);
  2. its `hypothesis.hash` is the file's, or `hypothesis_changed`;
  3. its `workload_hash` is the recorded file's, or `input_changed`, so an edit inside a batch is named as such;
  4. it is the directory asked for, its run_id is the one expected, and its `build_hash`, `engine_hash` and `seed` are the binary's and the file's, or `executor_mismatch`.

  For a reused bundle, check 4 aborts with `build_mismatch` instead (`bundle_invalid` for a run_id that is not its directory's).
- **Existing bundles first.** Every existing bundle a batch would use is found and checked before any of the batch runs (LOOP-10(f)), so an existing control from another build stops the batch before its treatment is made.
  - An existing bundle that does not verify aborts with `bundle_invalid`.
  - A directory with no manifest, left by a run that never finished, aborts with `bundle_incomplete`, and the message says to remove it. Staging bundles so that this cannot happen is `acn-trace`'s change, filed as issue #20.
- **The first batch** is always two bundles, a treatment and a control the loop has not yet made, so the budget's floor is 2.
  - A frozen file's floor is the grid: its cells plus their distinct controls.
  - A budget above 2^63 − 1 is refused with `budget_refused`, because the report writes it as an integer.
- **A frozen file declaring `random`** cannot load (HYP-9), so the runner's own check is defence in depth and has no test of its own.
- **`random`'s float draw** is `min + (max − min) × u`, with `u = (next_u64 >> 11) × 2⁻⁵³`, as LOOP-10(b) specifies.
  - The result is clamped to `[min, max]`, because `max − min` can round up and carry the sum past `max`, which the harness would refuse (HYP-6).
  - A `range` whose width `max − min` overflows a double is refused before anything runs (`search_refused`). Lint accepts such a range, but LOOP-10(b)'s formula cannot draw from it. Its known-answer vector has an enum, a `range`, an `int_range` and a bool; `loop_run::random_draws` exposes the draws for it.
- **The trajectory** computes a full verdict after every batch, so a loop over *n* cells costs *n* verdicts, each with its bootstrap.
  - That is cheap for the test grids, but quadratic for POC 4's full grid (1 536 cells).
  - LOOP-10(c) asks for it, and LOOP-11's report records it. Cheaper trajectories are a later spec question, not a reading to make here.

### The report (LOOP-11)
- **Canonical JSON.** `report.json` uses `verdict.json`'s writer (`json::J`).
- **Keys:**

  | Key | Contents |
  |---|---|
  | `format` | `acn-bench/loop-report/v1` |
  | `layer`, `loop_id` | |
  | `hypothesis` | `{id, status, hash, path}` |
  | `inputs` | `{workloads: {value: {path, hash}}, models: {value: profile}, strategy, budget}` |
  | `seed` | decimal text, as in the manifest |
  | `engine_hash`, `build_hash` | |
  | `batches` | `[{cell, run_ids, verdict, reasons}]`, which is both the parameter samples and the trajectory |
  | `bundles` | `[{run_id, bundle_digest}]`, ascending |
  | `stop` | why the loop stopped |
  | `verdict_id`, `verdict`, `reasons` | the final verdict |
  | `best`, `worst` | |
  | `control_effect` | `[{slice, cell, quantity, effect, ci_low, ci_high}]`; ADR-25 adds `treatment_replicates` and `control_replicates` (CON-18) |
  | `lab_note` | `{question, varied, observed, next_layer}` |

- **`next_layer`** is `L2` when `twin_required` holds and `L3` otherwise (ADR-22).
- **`report.md`** is rendered from `report.json`'s text by `loop_run::markdown` and nothing else, and the test checks exactly that.
- **`loop_exists`** is checked before anything runs, and again by the writer.
- **Writing order.** The report is rendered before the final verdict is written. The verdict and the report are then written one after the other, each after the inputs are re-read (LOOP-13). An input change caught by the second re-read still leaves the verdict on disk, contrary to LOOP-10(f)'s "neither", because LOOP-13 asks for a re-read before each write. Rendering first narrows that window to the re-read itself.

### Regeneration (LOOP-14)
- **The comparison.**
  - Bundles are compared by `bundle_digest`. Both copies verify, and the manifest names every file's hash, so equal digests mean equal bytes.
  - An original bundle that no longer verifies is reported as differing.
  - `report.json` and `report.md` are compared as bytes, and so is the final verdict's text against `runs/verdicts/<verdict_id>/verdict.json`.
- **The result.** `differ` names `report.json`, `report.md`, `verdict.json` or `bundle <run_id>`, and `ok` is true iff `differ` is empty (CON-8).
- **`input_changed`** covers any of:
  - a hypothesis whose hash or status moved;
  - a workload whose hash moved, or that can no longer be read;
  - inputs that give another `loop_id`.

  The recorded files are hashed before anything else reads them, so a deleted or edited input is named `input_changed`, never `workload_refused` or `pins_refused`.
- **The report path** is made canonical first, so `runs/` and the base are found however the path was given.

### Threat model
The loop protects against accidents and edits to its inputs, not against someone with write access to `runs/`:
- **Swapping paths.** Such a person can swap a directory the writer has checked for a symbolic link before the next level is created. Closing that needs directory handles (`openat`), which the standard library does not offer.
- **Planted bundles.** They can plant a bundle with fabricated traces under the run_id the loop expects, and `loop run` will reuse it, since every hash it checks can be computed by anyone.
- **What proves a report.** Only `--from-report`, which reuses nothing, proves a report: a planted bundle shows there as `bundle <run_id>` in `differ`.

### A spec gap found (HYP-9, HYP-20)
A file with `[design].pins` and no `provider` parameter can never be judged:
- `pins.models` may be keyed only by provider values (HYP-9's load check);
- but HYP-20's pin check looks up the backend (`mockllm`) when there is no provider.

The loop refuses such a file up front with `pins_refused`, as the verdict would refuse it. This is filed as issue #18 (`spec-conflict: HYP-9, HYP-20`) rather than read away.

## Consequences
- In scope: LOOP-3, 10, 11, 13, 14 and 15. HYP-4's loop half is implemented.
- T05b.2 adds LOOP-1, LOOP-2 and LOOP-4.
- POC 4's full grid needs a cheaper trajectory, or patience, before T06 runs it.
