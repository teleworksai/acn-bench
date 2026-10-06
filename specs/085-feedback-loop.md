# SPEC 085 — The layered, verifiable feedback loop

**Status:** Draft v0.4 (October 2026; v0.4 specifies L2: what `acn loop twin` runs, the twin object it writes, and how `acn evidence verify` walks it (LOOP-12, LOOP-16); v0.3 thins the verdict trajectory to at most fifty verdicts before the last (LOOP-10(c)), so a grid of a few thousand bundles runs in hours rather than days; v0.2 makes L1 implementable: the layer of an evidence object, the chain `acn evidence verify` walks, the runner's strategies, batches, stop rule, refusals and aborts, the loop report and its regeneration on the build that made it, and where decisions are made; `bisect` is deferred). **Inherits:** SPEC 000, 010, 080. **Prefix:** LOOP. **Crates:** `acn-hyp` (loop runner, frozen set), `acn-ctl`, `acn-cli`.
**Purpose:** define how a POC moves from an idea to a citable result as a loop of five layers, where each layer is verified by a machine check before its output can feed the next, where feedback flows only to artifacts a layer is allowed to change, and where the whole chain from a cited number back to source, seed and scenario is verifiable by one command.

## 0. Why layered

One loop that goes "run → look → edit → run" cannot be trusted at scale because nothing distinguishes a real improvement from an edit to the question. acn-bench separates the loop into layers by *time-scale* and by *what may change*. Fast layers change code and models; slow layers change hypotheses and specs; only the human layer changes what is frozen. Every layer emits an evidence object hashed to the one below, so "this number came from this seed, this scenario, this hypothesis, this commit" is a property you can check, not a claim.

## 1. The five layers

| Layer | Loop | Time-scale | Verifier (machine check) | May change | May NOT change |
|---|---|---|---|---|---|
| **L0 Build** | edit → gates | seconds–minutes | `tools/ci.sh`: fmt, clippy, tests, trace-check, env-hash, deny | code, tests, docs | specs, hypotheses |
| **L1 Sim** | run hypothesis in `sim` over parameter ranges → verdict | minutes | bit-identical re-run (CON-5c); control present (CON-18); parameters within declared ranges | parameters; candidate hypotheses (lab only); link/mock models with a lab note | frozen hypotheses; specs |
| **L2 Twin** | same scenario in `live` → sim↔live divergence | hours | divergence within tolerance (CON-25); run_id chain to L1 bundle | simulator calibration (link models, mock timing) via Class B PR | the hypothesis (a twin mismatch is never a reason to edit the question) |
| **L3 Reality** | real providers, measured traces, real inference nodes, `netem` | days | per-provider verdicts (CON-26); live↔netem agreement on measured traces; second-machine regeneration (T16) | trace library (Class C), cache-mapping table (TRC-21), mock models | frozen hypotheses; specs |
| **L4 Graduation** | lab note → human review → spec-change / freeze / cite → WG feedback → new candidates | weeks | adversarial review (CON-7); external reproduction from the kit (POC 16) | specs, frozen hypotheses, catalogue | — (human decision, recorded in `docs/gates/` and ADRs) |

**LOOP-1** Every evidence object MUST be produced by exactly one layer, and its layer MUST be derived from what it records, never asserted beside it. The evidence objects are bundles (TRC-22), verdicts (`runs/verdicts/<verdict_id>/verdict.json`, HYP-20), loop reports (`runs/loop/<loop_id>/report.json`, LOOP-11) and the L2 and L3 objects of LOOP-12. A bundle's layer is fixed by its manifest: `sim` (always on the mock) is L1; `live` on the mock is L2, the simulator's twin; `live` on a real provider is L3, and so is `netem` on either backend. A verdict's layer is the highest layer among its bundles, so a verdict over sim bundles and their live twins (HYP-22) is L2. A loop report is L1, and an L2 or L3 object has the layer of the bundles it adds. Where an object also writes its layer in a field `layer` (`"L1"`, `"L2"`, `"L3"`), the field is a copy of the derived layer, and `acn evidence verify` MUST fail when the two differ.

**LOOP-2** An evidence object MUST reference, by hash, the evidence objects it was derived from: a loop report lists the run_id and bundle_digest of every bundle and the verdict_id of its final verdict; an L2 or L3 object (LOOP-12) names the loop_id it extends and lists, per cell, the L1 run_ids its live bundles twin (`derived_from`). A cited number MUST resolve, through this chain, to L1 bundles whose run_ids regenerate (CON-5e). `acn evidence verify <loop_id | verdict_id>` MUST walk the chain and fail on any missing or non-regenerating link:
- every bundle MUST verify with its views recomputed (TRC-23, TRC-35);
- every verdict MUST be recomputed from its bundles, through the same `acn-hyp` function as `acn hyp verdict` (HYP-20), and be byte-identical to the file, with the same verdict_id (HYP-15);
- the loop report MUST regenerate as LOOP-14 says, which regenerates every L1 bundle it lists;
- every recorded `layer` MUST equal the derived one (LOOP-1).

A verdict_id resolves to the loop report under `runs/loop/` whose final verdict it is. A verdict that no report names has no recorded inputs to regenerate from, and verify fails on it with `no_loop_report`. Regeneration needs a binary whose `build_hash` equals the one the report records, because byte identity is claimed only within one build (CON-31); on any other build verify fails with `not_regenerable_with_this_build`, so evidence is verified on the build that made it. A live bundle cannot regenerate (CON-29): it is verified by its hashes, its `bundle_digest` and its recomputed views, never re-run. An L2 verdict_id resolves to the twin object `runs/loop/<loop_id>/twin/<verdict_id>/twin.json` that names it (LOOP-16), and is verified through that object.

**LOOP-3** Feedback direction is restricted: a result at layer *n* MAY cause changes only to the artifacts listed as changeable at layers ≤ *n*. In particular, L1–L3 results MUST NOT modify frozen hypotheses or specs; the loop runner and any automated agent MUST NOT hold write access to them (CON-7, CON-17). A verdict of `fail` on a frozen hypothesis is an L4 input, not an L1 edit.

**LOOP-4** A layer MUST NOT consume evidence that has not passed the verifier of the layer below. `acn loop` MUST refuse to start an L2 twin on an L1 bundle that does not regenerate (LOOP-14). It MUST refuse an L3 provider run for a hypothesis unless an L2 verdict exists whose live bundles twin every decision cell of the L1 final verdict (`decision_cells` in `verdict.json`, HYP-22) and whose reasons do not include `twin_failed` (HYP-21). A verdict with no live bundle, or one that twins only some of the decision cells, does not pass. The L3 check is waived only when the hypothesis file declares `twin_required = false`, which is permitted only for network-free hypotheses such as POC 4. These two gates are checks the loop runner exposes and tests on their own; the `twin` and `promote` commands of LOOP-12 call them.

## 2. The loop runner

**LOOP-10** `acn loop run --hypothesis <file> --workload <file> --model <profile> --budget <n>` MUST execute L1 for a candidate or frozen hypothesis as (a) to (f) say. It runs:
- in `sim`, on the mock, with `--model` naming the mock profile (MLM-50);
- with the hypothesis's run seed (HYP-9);
- with every run option at its default, so no `opt.` parameter is set (HAR-25, CON-29);
- with no scenario: `scenario_hash` is zero (ADR-17).

Paths resolve against the workspace root (CON-28). Two inputs can be given per parameter value:
- When the file varies a parameter named `workload`, `--workload` MUST instead be given once per value as `<value>=<file>`, and each cell runs on the file for its value.
- When the file varies a parameter named `provider`, `--model` MAY be given once per value as `<value>=<profile>`.

A map MUST name every value of its parameter and nothing else.
- (a) **Batches.** The loop proceeds one *batch* at a time. A batch is one cell of the design, made of two bundles:
  - the treatment bundle of that cell, holding all `[design].replicates` replicates;
  - unless the loop has already made it, the control bundle of the effective configuration the cell maps to (HYP-8).

  Parameter values are those of the cell, always within the declared domains (HYP-6). Every bundle is made by the executor of LOOP-15.
- (b) **Strategies.** The strategy is the file's `[design].search`, and no option selects another.
  - `grid` runs every cell of every slice, slices in key order and cells in HYP-14 order.
  - `random` is for candidates only (HYP-9). It draws each batch's cell from the sub-stream `loop.search` of the run seed (CON-30b), one parameter at a time in bytewise name order:
    - for a `bool` (`false`, `true`), an `enum` (its values in file order) or a parameter that declares `levels` (in file order), an index drawn with the ranged-integer sampler CON-5(a) pins (ADR-19);
    - for an `int_range` without levels, an integer drawn the same way over `[min, max]`;
    - for a `range` without levels, `min + (max − min) × u`, where `u` is the generator's next 64-bit output shifted right by 11 and multiplied by 2⁻⁵³.

    A cell already run is drawn again, and 1 000 consecutive redraws exhaust the strategy. The PR that implements `random` adds a known-answer vector of its first draws (CON-27(d)).
  - `bisect` is not specified yet (§6 question 2), and a file whose `search` is `bisect` is refused.
- (c) **Verdict trajectory.** After every k-th batch, with k = ⌈budget / 50⌉, and after the last batch, the verdict of HYP-20..24 MUST be computed over every bundle of the batches run so far, by the same `acn-hyp` function `acn hyp verdict` uses (HYP-20). Its value and reasons are appended to the trajectory, and the batches between record none.
  - The verdict's cost grows with the bundles it reads, so a verdict after every batch makes a loop's cost quadratic in its grid.
  - k depends on the budget alone. The budget is an input of `loop_id`, so k is fixed before the loop starts, and the trajectory is part of what a report regenerates (LOOP-14).
  - A budget of 50 or fewer keeps a verdict after every batch.

  These intermediate verdicts are not written; the final verdict is written once, under `runs/verdicts/` (HYP-20). Two loops can end on the same bundle set:
  - if `runs/verdicts/<verdict_id>/` already exists and its `verdict.json` is byte-identical to the one computed, it is referenced;
  - otherwise the loop aborts with `verdict_conflict`.
- (d) **Stop rule.** The loop MUST stop when the next batch would exceed the budget or when the strategy has no cell left (every grid cell run, or the random draws exhausted).
  - It MUST NOT stop on the value of the verdict: stopping when a result looks good is optional stopping, which HYP-21's fixed replicate set exists to prevent.
  - The budget counts the bundles of the batches run, treatment and control alike. A bundle reused under LOOP-11 counts exactly as one made.
  - The loop stops at the first batch that does not fit and never skips ahead to a cheaper one.
  - A budget in minutes is not accepted at L1, because the report must regenerate (LOOP-14).
- (e) **Refusals.** The loop MUST refuse each of the following, with a named reason, before it runs anything:
  - a frozen hypothesis whose `search` is not `grid`, and any file whose `search` is `bisect` (HYP-9);
  - a file with no `[control]`: HYP-8 lets a candidate omit it, but a loop without a control measures nothing (CON-18);
  - a `workload` control, until the generator modes of SPEC 050 exist;
  - a file whose `[design].backends` does not admit the mock (HYP-9; none named means the mock only);
  - a `--model` that names no mock profile (MLM-50);
  - a workload or model map that does not name exactly its parameter's values;
  - a workload file that does not load;
  - when the file has `[design].pins`, every pin check of HYP-20 that can be decided before running:
    - a workload hash not among `pins.workload`;
    - a zero scenario hash not among `pins.scenario`;
    - a cell whose profile differs from `pins.models` for its `provider` value, or for `mockllm` when the file varies no `provider`;
  - a budget smaller than the first batch;
  - for a frozen hypothesis, a budget smaller than the whole grid, because a frozen verdict reads every cell (HYP-21).
- (f) **Aborts.** The loop aborts, naming the reason, on any of:
  - a verdict refused mid-loop (HYP-20);
  - an existing bundle from another build (LOOP-11), checked before the batch that would use it;
  - an executor that fails or returns another bundle than the one expected (LOOP-15);
  - a changed input (LOOP-13);
  - a verdict conflict (c).

  The bundles already made stay, and neither a report nor a final verdict is written.

**LOOP-11** On completion, `acn loop run` MUST write a **loop report** (`runs/loop/<loop_id>/report.json` and a Markdown rendering, `report.md`, made from the JSON alone) and print one JSON object with `ok`, `loop_id`, `report`, `verdict_id` and `run_ids` (CON-8). The report contains:
- hypothesis id/status/hash;
- parameter samples and the verdict trajectory;
- the best and worst configurations, and the control effect;
- the run_ids of every bundle;
- the machine-generated draft of a lab note: the question, what was varied, what was observed, and the suggested next layer.

The loop runner MUST NOT write anything outside `runs/`.
- `loop_id = blake3("acn-bench/loop_id/v1\0" ‖ hypothesis_hash ‖ scenario_hash ‖ engine_hash ‖ hyp_status ‖ workloads ‖ models ‖ strategy ‖ budget)`, encoded as CON-27 says.
  - `hyp_status` and `strategy` are strings, and `budget` is an unsigned 64-bit integer.
  - `workloads` is the number of its entries as an unsigned 32-bit integer, then each entry in bytewise order of its value: the value as a string, then the workload file's hash. A single `--workload` file is one entry whose value is empty.
  - `models` is written the same way, with the profile as a string in place of the hash.

  `loop_id` depends on hashes, not on paths, and it does not depend on the build, which the report records (CON-31). The PR that implements it adds a known-answer vector (CON-27(d)).
- The report is JSON in the same form as `verdict.json` (HYP-15: sorted keys, numbers in CON-27(c) form, one trailing newline), with `"format": "acn-bench/loop-report/v1"`. Besides the contents above it records:
  - `layer` (`"L1"`) and the `loop_id`;
  - the inputs: the hypothesis path and every workload path relative to the workspace root, with their hashes, the model map, the strategy and the budget;
  - the run seed, `engine_hash` and `build_hash`;
  - per batch, the cell, the run_ids of its bundles and, where LOOP-10(c) computes one, the verdict after it and its reasons, both `null` otherwise;
  - why the loop stopped;
  - the verdict_id of the final verdict.

  It does not record which bundles were reused, so that a regeneration, which reuses none (LOOP-14), writes the same bytes.
- The *best* and *worst* configurations are the cells with the largest and the smallest effect of the first quantity of `[measures].primary`, over every slice, ties broken by slice key and then by HYP-14 order. Cells whose effect is undefined are left out, and when none is left both are null. The *control effect* is the treatment-minus-control effect of every primary quantity in every cell run (CON-18).
- If `runs/loop/<loop_id>/report.json` exists, it is never overwritten: the loop refuses to start, with `loop_exists`.
- If a bundle the loop would make already exists, it is verified (TRC-23) and used instead of being re-run, but only when its manifest's `build_hash` and `engine_hash` equal the binary's, since only then does the same run_id regenerate the same bytes (CON-5c, CON-31). Otherwise the loop aborts with `build_mismatch`, naming the bundle; a re-run after a code change starts from a `runs/` without it.

**LOOP-12** `acn loop twin --loop <loop_id> [--top k]` MUST execute L2 for a loop report, as follows:
- **Gate.** It first passes the L2 gate of LOOP-4: the report regenerates byte for byte (LOOP-14), or the twin is refused with `twin_refused`, naming the regeneration's own failure (`input_changed`, `not_regenerable_with_this_build`, or the files that differ), and nothing is run. An unknown `loop_id` is refused with `no_loop_report`. The gate re-runs the whole L1 loop, so a twin costs at least as much as its loop.
- **Cells.** The loop runner chooses the cells to twin from the report and its final verdict, and from nothing else (LOOP-15). They are the union of:
  - the decision cells of every slice (`decision_cells`, HYP-22);
  - the *k* best and the *k* worst cells. Every cell of every slice whose effect of the first quantity of `[measures].primary` is defined is ranked by that effect, the value the report records (LOOP-11): descending for the best, ascending for the worst, ties broken in both by slice key and then by HYP-14 order. When fewer than *k* such cells exist, all of them are taken. With these rules the first best and first worst are the report's own.

  `k` is any integer ≥ 0 and defaults to 1, so the report's best and worst are always twinned; `--top 0` twins the decision cells alone. The union is ordered by slice key and then by HYP-14 order, each cell once, with every reason it was chosen (`decision`, `best`, `worst`, in that order). A report with no cell to twin is refused with `nothing_to_twin`.
- **Runs.** Each distinct L1 bundle of a chosen cell, treatment or control, is twinned once, so a control shared by several cells (LOOP-10(a)) runs once. Its twin is the executor's run of the same request in `live` (LOOP-15): the same hypothesis, workload, model, parameter values, arm and replicate count, so replicate seeds and indices pair by index (HYP-22). The mock is served by the harness, a fresh one per replicate (HAR-26), and every other run option is at its default (LOOP-10). Without a scenario in the L1 bundles (`scenario_hash` zero), the live runs have none either (HYP-20). The bundles run one at a time in the order of the cells, treatment before control; within a run the harness orders replicates by `run.order` (CON-5(d)).
- **Where.** The live bundles go under `runs/live/<n>/<run_id>/`, `n` being the smallest positive decimal not yet used. A live bundle is a measurement and is never reused: every twin measures afresh, and two twins of one loop are two objects (LOOP-16).
- **Verdict.** It computes the L2 verdict over exactly the L1 final verdict's bundles and the live bundles, through the function of HYP-20, and that verdict records the divergence per quantity (HYP-22). It checks LOOP-16's `twin_exists` before writing anything under `runs/verdicts/` or `runs/loop/`. It then writes the verdict as `acn hyp verdict` would, except that an existing `verdict.json` of the same id that is byte-identical is kept and referenced, and one that differs fails the twin with `verdict_conflict`.
- **Object.** It writes the twin object of LOOP-16 and prints one JSON object (CON-8) with:
  - `ok`;
  - `loop_id`;
  - `twin`, the object's path;
  - `verdict_id`;
  - `run_ids`, the live bundles, ascending;
  - `twinned`, true when every decision cell is twinned;
  - `twin_failed`.

  `ok` reports that the twin completed, whatever the divergence. A simulator shown to diverge is a result: when the hypothesis requires a twin, it is recorded as `twin_failed` in the verdict (HYP-21, HYP-22) and stops L3 at LOOP-4's gate. Otherwise the divergences are recorded and nothing more. An executor failure, a mock that cannot be served included, aborts with `executor_failed` and writes no object.

`acn loop promote --loop <loop_id> --provider <name>` executes L3 for one provider and writes its per-provider verdicts as a new L3 object under `runs/loop/<loop_id>/promote/<provider>/`. Its format lands with real providers, T30.

Each of these objects is a separate file with its own `layer` and `derived_from` (LOOP-1, LOOP-2); `report.json` and `report.md` are never changed.

**LOOP-16** The twin object MUST be written to `runs/loop/<loop_id>/twin/<verdict_id>/twin.json`, the verdict_id being the L2 verdict's, as JSON in the form of `verdict.json` (HYP-15) with `"format": "acn-bench/loop-twin/v1"`. If the file exists it is never overwritten, and the twin fails with `twin_exists`. Every field is a function of the report, the L2 verdict, the live bundles and `top`. It records:
- `layer`, a copy of the derived layer: `"L2"`, because its live bundles run on the mock (LOOP-1);
- the `loop_id` it extends and the L1 final verdict's `verdict_id`;
- `top`, the *k* it ran with, and `live_dir`, the directory of its live bundles relative to `runs/` (`live/<n>`);
- per chosen cell, in the order of LOOP-12, the cell's:
  - slice key and parameter values;
  - reasons;
  - per arm, the L1 run_id it twins (`derived_from`) and the live bundle's run_id and bundle_digest;
- the L2 `verdict_id`, its verdict and reasons, and its twin label (none or `partially-twinned`, HYP-22);
- per slice, per twinned cell, per quantity with a tolerance: the tolerance, the treatment, control and effect divergences (`null` where undefined or not measured), and whether all are within it. These are copied from the L2 `verdict.json`, not computed a second time;
- the `engine_hash` and `build_hash` of the binary that ran it.

`acn evidence verify <loop_id>` MUST also walk every twin object under `runs/loop/<loop_id>/twin/`, and verifying an L2 verdict_id walks the one object that names it (LOOP-2). The walk fails unless:
- the directory's name is the object's L2 verdict_id;
- the object's `layer` equals the derived one;
- its `loop_id` and L1 verdict_id are the report's, and the report verifies (LOOP-2);
- the cells, their reasons and their `derived_from` are those LOOP-12 chooses from the report and `top`;
- each live bundle, read from `live_dir`, verifies with its views recomputed (TRC-23, TRC-35);
- each live bundle's run_id is the one CON-29 gives for its L1 bundle's inputs in `live` with the endpoint of HAR-26, so it twins that bundle;
- the L2 verdict's bundle set is exactly the L1 final verdict's bundles and the live bundles;
- the L2 verdict recomputes byte for byte (HYP-15);
- `twin.json` regenerates byte for byte from the report, the L2 verdict, the live bundles and `top`.

A live bundle is not re-run: it is a measurement, and its chain ends in the L1 bundles it twins (LOOP-2).

**LOOP-13** The loop runner MUST treat the hypothesis file and the workload files as read-only input and MUST record the hypothesis hash in every bundle. If the hypothesis file changes between batches, the loop MUST abort with `hypothesis_changed`; if a workload file changes, it MUST abort with `input_changed`. The files are re-read:
- before every batch;
- before the final verdict is written;
- before the report is written.

The runner also checks each returned bundle's `hypothesis.hash` (LOOP-15). An abort writes no report.

**LOOP-14** Parameter search strategies MUST be deterministic given the loop seed (the sub-stream `loop.search` of the run seed, CON-30b). A loop report MUST be regenerable by `acn loop run --from-report <report>`, which:
- refuses with `input_changed` when the hypothesis or a workload file at its recorded path no longer has its recorded hash;
- refuses with `not_regenerable_with_this_build` when the binary's `build_hash` differs from the report's (CON-31);
- re-runs the loop from the inputs the report records, into a fresh directory `runs/regen/<loop_id>/<n>/`, laid out as `runs/` is (bundles at `<run_id>/`, the report at `loop/<loop_id>/`):
  - `n` is the smallest positive decimal not yet used;
  - no bundle is reused;
  - verdicts are computed in memory;
- compares every bundle, `report.json`, `report.md` and the final verdict's `verdict.json` byte for byte with the originals;
- prints one JSON object whose `ok` is true if and only if all of them are identical, and which lists any that differ (CON-8);
- writes nothing outside its directory.

A regeneration copy is a scratch artifact, not a TRC-22 bundle: it is never cited or read as evidence, and it may be deleted.

**LOOP-15** Every decision of the loop MUST be made by the loop runner in `acn-hyp`, from the hypothesis file and the bundles alone. That covers which cell runs next, when the loop stops, every verdict, and every word of the report.

The bundles are produced by an executor the runner calls. `acn-cli` supplies the executor with the harness (SPEC 040), and `acn-hyp` does not depend on the harness.

For each bundle, the runner passes the executor exactly the inputs of HAR-50, which derives the seed from the hypothesis, and the directory the bundle goes under:
- the hypothesis path;
- the workload file;
- the model;
- the cell's parameter values;
- the arm;
- the replicate count;
- the mode, `sim` at L1, and at L2 `live` with `opt.endpoint = "acn-mock://loopback"` (HAR-26) and every other option at its default.

The executor returns a bundle directory. The runner MUST verify each returned bundle (TRC-23) and check its manifest against what it expected: the run_id, which it computes itself (CON-29), the seed, `hypothesis.hash`, `engine_hash` and `build_hash`. On a failure the loop aborts:
- an executor error aborts with `executor_failed`;
- a changed `hypothesis.hash` aborts with `hypothesis_changed` (LOOP-13);
- any other difference aborts with `executor_mismatch`.

So the frozen code decides, and the run path only runs.

## 3. Where the agent sits

**LOOP-20** An auto-research agent (Claude Code or any other) interacts with the loop only through `acn loop` and `acn ctl` and through lab artifacts: it MAY edit candidate hypotheses in `lab/hypotheses/`, propose Class A/B PRs, write lab notes from loop reports, and draft `spec-change` PRs for a human to merge. It MUST NOT be granted credentials or filesystem permissions that reach `hypotheses/`, `specs/`, `scenarios/measured/` or `crates/acn-hyp/`; CI MUST verify CODEOWNERS covers these paths.

**LOOP-21** The agent's own sessions on the loop MUST be recorded as an `acn.session` (SPEC 010) with `acn.role = "researcher"` so that POC 9 can measure them.

## 4. Evidence chain and reporting

**LOOP-30** `docs/evidence/<hypothesis-id>.md` MUST be regenerated by `cargo xtask docs-inventory` from loop reports and gate documents: one page per hypothesis showing the current verdict at each layer, the divergence figures, the provider table, and the chain of run_ids. This page is the only citation target for the ACN report and WG contributions.

**LOOP-31** A gate document (`docs/gates/M<n>.md`, CON-22) MUST cite evidence pages, not raw bundles, and `acn evidence verify --gate M<n>` MUST pass before the gate PR is merged.

## 5. Acceptance tests

- `crates/acn-hyp/tests/loop_direction.rs` — LOOP-3, HYP-4: given write access to a temp copy of `hypotheses/`, the runner never writes there, and a mutation attempt is refused.
- `crates/acn-hyp/tests/loop_gating.rs` — LOOP-4:
  - a twin is refused on a non-regenerating L1 bundle;
  - a provider run is refused on an out-of-tolerance twin, on a verdict with no live bundle, and on one that twins only some of the decision cells.
- `crates/acn-hyp/tests/loop_replay.rs` — LOOP-14:
  - a report regenerates byte-identically;
  - a differing bundle makes `ok` false;
  - a changed input and another `build_hash` are refused;
  - a second regeneration takes the next `n`;
  - nothing is written outside the regeneration directory.
- `tests/accept/evidence_chain.rs` — LOOP-1, LOOP-2:
  - an L1 chain (report → verdict → bundles) verifies;
  - verification fails on a broken hash, on a recorded `layer` that differs from the derived one, and on a verdict no report names.
  - an L2 chain (twin object → L2 verdict → live bundles → the L1 bundles they twin) verifies, and fails on a twin object whose `derived_from` names a bundle the report does not, or whose L2 verdict does not recompute.
  - The L3 links are added with T30, and the evidence pages (LOOP-30) with T07.
- `crates/acn-hyp/tests/loop_twin.rs` — LOOP-12, LOOP-16:
  - the cells chosen are the decision cells and the *k* best and worst, once each, in order;
  - a twin on a report that does not regenerate is refused before any run;
  - a shared control is run once, and every live bundle goes under a fresh `runs/live/<n>/`;
  - the twin object records the pairs, the divergence and the derived layer, regenerates from its inputs, and is never overwritten;
  - an existing identical L2 verdict is kept, and a differing one fails with `verdict_conflict`.
- `crates/acn-hyp/tests/hypothesis_changed.rs` — LOOP-13, HYP-4.
- `crates/acn-hyp/tests/loop_run.rs` — LOOP-1, LOOP-10, LOOP-11, LOOP-15:
  - the layer derived for each kind of object;
  - each strategy's cells and stop rule, including the budget boundary with reused bundles counted;
  - every refusal and every abort, the executor mismatches included;
  - the trajectory, and the rule for a final verdict that already exists;
  - the report's fields;
  - the known answers for `loop_id` and `random`;
  - an existing bundle reused on the same build and refused from another;
  - an existing report never overwritten.

## 6. Open questions (ADR candidates)

1. Whether L2 tolerance should be per-quantity in the hypothesis file (current) or a global default with per-hypothesis overrides.
2. `bisect` is deferred until SPEC 080 §6 question 2 defines a boundary. When it returns it needs:
   - the values held for the parameters it does not search;
   - its levels, midpoint and termination;
   - a restricted boundary predicate.

   It should not be allowed to propose *new* parameter axes: that is a candidate-hypothesis edit, and it belongs to the agent at L4-lite in lab.
3. How much of the loop report to publish with the kit: proposal is all of it, with provider raw responses redacted (TRC-42).
4. SPEC 080 §6 question 5 asked for LOOP-10's stop rule to be restated in terms SPEC 080 defines; LOOP-10(d) does so (budget or strategy exhausted, never the verdict's value). Whether an adaptive design may ever stop early on a frozen hypothesis stays with SPEC 080 §6 question 2.
5. The best/worst configuration reads the first primary quantity only; a hypothesis with several primary quantities may want one per quantity.
6. The rule for the lab-note draft's suggested next layer is left to the implementation (ADR-22).
