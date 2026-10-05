# SPEC 085 — The layered, verifiable feedback loop

**Status:** Draft v0.2 (October 2026; v0.2 makes L1 implementable: the layer of an evidence object, the chain `acn evidence verify` walks, the runner's strategies, batches and stop rule, the loop report and its regeneration, and where decisions are made). **Inherits:** SPEC 000, 010, 080. **Prefix:** LOOP. **Crates:** `acn-hyp` (loop runner, frozen set), `acn-ctl`, `acn-cli`.
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

**LOOP-1** Every evidence object MUST be produced by exactly one layer, and its layer MUST be determined by what it records, never asserted beside it. The evidence objects are bundles (TRC-22), verdicts (`runs/verdicts/<verdict_id>/verdict.json`, HYP-20) and loop reports (`runs/loop/<loop_id>/report.json`, LOOP-11). A bundle's layer is fixed by its manifest: `sim` (always on the mock) is L1; `live` on the mock is L2, the simulator's twin; `live` on a real provider, and `netem`, are L3. A verdict's layer is that of the bundles its quantities come from (HYP-21). A loop report records its layer in the field `layer` (`"L1"`, `"L2"`, `"L3"`).

**LOOP-2** An evidence object at layer *n* MUST reference, by hash, the evidence objects at layer *n−1* it was derived from: a loop report lists the run_id and bundle_digest of every bundle and the verdict_id of every verdict it made, and a section that LOOP-12 appends for L2 or L3 lists, per cell, the L1 run_ids its live bundles twin (`derived_from`). A cited number MUST resolve, through this chain, to L1 bundles whose run_ids regenerate (CON-5e). `acn evidence verify <loop_id | verdict_id>` MUST walk the chain and fail on any missing or non-regenerating link: every bundle MUST verify with its views recomputed (TRC-23, TRC-35); every verdict MUST be recomputed from its bundles and be byte-identical to the file, with the same verdict_id (HYP-15); every L1 bundle MUST regenerate, re-run from the inputs the report records into a scratch directory under `runs/` and byte-identical to the original (CON-5c); and a loop report MUST regenerate as LOOP-14 says. A live bundle cannot regenerate (CON-29) and is verified by its hashes and its `bundle_digest` alone.

**LOOP-3** Feedback direction is restricted: a result at layer *n* MAY cause changes only to the artifacts listed as changeable at layers ≤ *n*. In particular, L1–L3 results MUST NOT modify frozen hypotheses or specs; the loop runner and any automated agent MUST NOT hold write access to them (CON-7, CON-17). A verdict of `fail` on a frozen hypothesis is an L4 input, not an L1 edit.

**LOOP-4** A layer MUST NOT consume evidence that has not passed the verifier of the layer below. `acn loop` MUST refuse to start an L2 twin on an L1 bundle that does not re-run bit-identically, and MUST refuse an L3 provider run for a hypothesis whose L2 divergence is out of tolerance (a verdict whose reasons include `twin_failed`, or that has no live bundle, HYP-21, HYP-22), unless the hypothesis file declares `twin_required = false` (permitted only for network-free hypotheses such as POC 4). These two gates are checks the loop runner exposes and tests on their own; the `twin` and `promote` commands of LOOP-12 call them.

## 2. The loop runner

**LOOP-10** `acn loop run --hypothesis <file> --workload <file> --model <profile> --budget <n>` executes L1 for a candidate or frozen hypothesis: in `sim`, on the mock (`--model` names its profile, MLM-50), with the hypothesis's run seed (HYP-9).
- (a) **Batches.** The loop proceeds one *batch* at a time. A batch is one cell of the design: the treatment bundle of that cell, holding all `[design].replicates` replicates, and, unless the loop has already made it, the control bundle of the effective configuration the cell maps to (HYP-8). Parameter values are those of the cell, always within the declared domains (HYP-6).
- (b) **Strategies.** `grid` runs every cell of every slice, slices in key order and cells in HYP-14 order. `random` and `bisect` are for candidates only (HYP-9). `random` draws each batch's cell from the sub-stream `loop.search` of the run seed (CON-30b): a `bool` or `enum` value uniformly, a `range` or `int_range` value uniformly over its `levels` when it declares them and otherwise over `[min, max]` (an integer for an `int_range`); a cell already run is drawn again, and 1 000 consecutive redraws exhaust the strategy. `bisect --axis <param> --boundary <predicate>` looks for the value of one numeric pooled parameter at which a per-cell boolean predicate in the language of HYP-10 (no `at` clause, no aggregate) changes value: every other parameter takes its control-config value, or else its first value (HYP-14 order); it runs the cells at `min` and `max`, stops if the predicate has the same value at both, and otherwise runs the midpoint (rounded down for an `int_range`) and keeps the half whose ends differ, until the interval is no wider than `--resolution` (default 1 for an `int_range`, `(max − min) / 1024` for a `range`). A cell whose predicate is undefined ends the search.
- (c) **Verdict trajectory.** After each batch the verdict of HYP-20..24 is computed over every bundle the loop has made, and its value and reasons are appended to the trajectory. These intermediate verdicts are not written; the final verdict is written once, under `runs/verdicts/` (HYP-20).
- (d) **Stop rule.** The loop stops when the budget is spent or the strategy has no cell left (every grid cell run; the bisection resolved or without a boundary; the random draws exhausted), and never on the value of the verdict: stopping when a result looks good is optional stopping, which HYP-21's fixed replicate set exists to prevent. The budget counts bundles, treatment and control alike; a batch that would exceed it is not started. A budget in minutes is not accepted at L1, because the report must regenerate (LOOP-14).
- (e) **Refusals.** The loop refuses, before it runs anything: a frozen hypothesis whose `search` is not `grid` (HYP-9); a file whose `[design].backends` does not admit the mock (HYP-9; none named means the mock only); a `workload` control, until the generator modes of SPEC 050 exist; and a workload whose hash is not among `[design].pins.workload` when the file has pins.

**LOOP-11** On completion, `acn loop run` MUST write a **loop report** (`runs/loop/<loop_id>/report.json` and a Markdown rendering, `report.md`, made from the JSON alone) containing: hypothesis id/status/hash, parameter samples and verdict trajectory, the best and worst configurations, the control effect, the run_ids of every bundle, and the machine-generated draft of a lab note (question, what was varied, what was observed, suggested next layer). The loop runner MUST NOT write anything outside `runs/`.
- `loop_id = blake3("acn-bench/loop_id/v1\0" ‖ hypothesis_hash ‖ workload_hash ‖ engine_hash ‖ inputs)`, `inputs` being the strings `model`, `strategy`, `budget` (decimal) and, for `bisect`, `axis`, `boundary` and `resolution` (CON-27(c)), in that order (CON-27). It does not depend on the build, which the report records (CON-31).
- The report is JSON as `verdict.json` is (HYP-15: sorted keys, numbers in CON-27(c) form, one trailing newline), with a `format` key. Besides the contents above it records: `layer` (`"L1"`); the inputs (the workload path as given and its hash, the model, the strategy and its arguments, the budget); the run seed, `engine_hash` and `build_hash`; per batch, the cell, the run_ids it made and the verdict after it; why the loop stopped; and the verdict_id of the final verdict.
- The *best* and *worst* configurations are the cells with the largest and the smallest effect of the first primary quantity, ties broken by HYP-14 order; the *control effect* is the treatment-minus-control effect of every primary quantity in every cell run (CON-18).
- A directory `runs/loop/<loop_id>/` that exists is never overwritten. A bundle whose run_id the loop would make and that already exists is verified (TRC-23) and used instead of being re-run: in `sim` the same run_id regenerates the same bytes (CON-5c).

**LOOP-12** `acn loop twin --loop <loop_id> [--top k]` executes L2 for the *k* configurations the report marks as decision-relevant (those nearest the falsifier boundary and the best/worst), records divergence per quantity, and appends to the report. `acn loop promote --loop <loop_id> --provider <name>` executes L3 for one provider and appends per-provider verdicts. (These land with the live twin, T11b, and real providers, T30.)

**LOOP-13** The loop runner MUST treat the hypothesis file as read-only input and MUST record its hash in every bundle; if the file changes between batches, the loop MUST abort with `hypothesis_changed`. The file is re-read before every batch, before the final verdict is written and before the report is written; an abort writes no report.

**LOOP-14** Parameter search strategies MUST be deterministic given the loop seed (the sub-stream `loop.search` of the run seed, CON-30b); a loop report MUST be regenerable by `acn loop run --from-report <report>`, which re-runs the loop from the inputs the report records, with its bundles under `runs/loop/<loop_id>/regen/`, compares every bundle and the report byte for byte with the originals, prints whether they are identical, and writes nothing else.

**LOOP-15** Every decision of the loop — which cell runs next, when the loop stops, every verdict, and every word of the report — MUST be made by the loop runner in `acn-hyp` from the hypothesis file and the bundles alone. The bundles are produced by an executor the runner calls, which `acn-cli` supplies with the harness (SPEC 040); an executor only turns a cell, an arm and a run seed into a CON-29 bundle, so that the frozen code decides and the run path only runs.

## 3. Where the agent sits

**LOOP-20** An auto-research agent (Claude Code or any other) interacts with the loop only through `acn loop` and `acn ctl` and through lab artifacts: it MAY edit candidate hypotheses in `lab/hypotheses/`, propose Class A/B PRs, write lab notes from loop reports, and draft `spec-change` PRs for a human to merge. It MUST NOT be granted credentials or filesystem permissions that reach `hypotheses/`, `specs/`, `scenarios/measured/` or `crates/acn-hyp/`; CI MUST verify CODEOWNERS covers these paths.

**LOOP-21** The agent's own sessions on the loop MUST be recorded as an `acn.session` (SPEC 010) with `acn.role = "researcher"` so that POC 9 can measure them.

## 4. Evidence chain and reporting

**LOOP-30** `docs/evidence/<hypothesis-id>.md` MUST be regenerated by `cargo xtask docs-inventory` from loop reports and gate documents: one page per hypothesis showing the current verdict at each layer, the divergence figures, the provider table, and the chain of run_ids. This page is the only citation target for the ACN report and WG contributions.

**LOOP-31** A gate document (`docs/gates/M<n>.md`, CON-22) MUST cite evidence pages, not raw bundles, and `acn evidence verify --gate M<n>` MUST pass before the gate PR is merged.

## 5. Acceptance tests

- `crates/acn-hyp/tests/loop_direction.rs` — LOOP-3: the runner, given write access to a temp copy of `hypotheses/`, never writes there; a mutation attempt is refused.
- `crates/acn-hyp/tests/loop_gating.rs` — LOOP-4: twin refused on a non-reproducible L1 bundle; provider run refused on out-of-tolerance twin.
- `crates/acn-hyp/tests/loop_replay.rs` — LOOP-14: report regenerates from seed.
- `tests/accept/evidence_chain.rs` — LOOP-2, LOOP-30: a synthetic three-layer chain verifies; breaking any hash fails.
- `crates/acn-hyp/tests/hypothesis_changed.rs` — LOOP-13.
- `crates/acn-hyp/tests/loop_run.rs` — LOOP-10, LOOP-11, LOOP-15: each strategy's cells and stop rule, refusals, the trajectory, the report's fields, `loop_id`'s known answer, an existing bundle reused, an existing report never overwritten.

## 6. Open questions (ADR candidates)

1. Whether L2 tolerance should be per-quantity in the hypothesis file (current) or a global default with per-hypothesis overrides.
2. Whether `bisect` should be allowed to propose *new* parameter axes (it should not; that is a candidate-hypothesis edit and belongs to the agent at L4-lite in lab).
3. How much of the loop report to publish with the kit: proposal is all of it, with provider raw responses redacted (TRC-42).
4. SPEC 080 §6 question 5 asked for LOOP-10's stop rule to be restated in terms SPEC 080 defines; LOOP-10(d) does so (budget or strategy exhausted, never the verdict's value). Whether an adaptive design (`bisect`, `random`) may ever stop early on a frozen hypothesis stays with SPEC 080 §6 question 2.
5. The best/worst configuration reads the first primary quantity only; a hypothesis with several primary quantities may want one per quantity.
