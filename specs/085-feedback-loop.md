# SPEC 085 — The layered, verifiable feedback loop

**Status:** Draft v0.1 (September 2026). **Inherits:** SPEC 000, 010, 080. **Prefix:** LOOP. **Crates:** `acn-hyp` (loop runner, frozen set), `acn-ctl`, `acn-cli`.
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

**LOOP-1** Every evidence object MUST be produced by exactly one layer and MUST name the layer (`acn.loop.layer`) in its manifest.

**LOOP-2** An evidence object at layer *n* MUST reference, by hash, the evidence objects at layer *n−1* it was derived from (`derived_from: [run_id…]`). A cited number MUST resolve, through this chain, to L1 bundles whose run_ids regenerate (CON-5e). `acn evidence verify <id>` MUST walk the chain and fail on any missing or non-regenerating link.

**LOOP-3** Feedback direction is restricted: a result at layer *n* MAY cause changes only to the artifacts listed as changeable at layers ≤ *n*. In particular, L1–L3 results MUST NOT modify frozen hypotheses or specs; the loop runner and any automated agent MUST NOT hold write access to them (CON-7, CON-17). A verdict of `fail` on a frozen hypothesis is an L4 input, not an L1 edit.

**LOOP-4** A layer MUST NOT consume evidence that has not passed the verifier of the layer below. `acn loop` MUST refuse to start an L2 twin on an L1 bundle that does not re-run bit-identically, and MUST refuse an L3 provider run for a hypothesis whose L2 divergence is out of tolerance, unless the hypothesis file declares `twin_required = false` (permitted only for network-free hypotheses such as POC 4).

## 2. The loop runner

**LOOP-10** `acn loop run --hypothesis <file> --budget <runs|minutes>` executes L1 for a candidate or frozen hypothesis: it samples parameters within the declared ranges using the declared strategy (`grid`, `bisect` for boundary-finding hypotheses such as POC 1a and 7, `random` with a seed), runs treatment and control per replicate, computes the verdict after each batch, and stops at budget exhaustion or when the falsifier's confidence interval no longer straddles its threshold.

**LOOP-11** On completion, `acn loop run` MUST write a **loop report** (`runs/loop/<loop_id>/report.json` and a Markdown rendering) containing: hypothesis id/status/hash, parameter samples and verdict trajectory, the best and worst configurations, the control effect, the run_ids of every bundle, and the machine-generated draft of a lab note (question, what was varied, what was observed, suggested next layer). The loop runner MUST NOT write anything outside `runs/`.

**LOOP-12** `acn loop twin --loop <loop_id> [--top k]` executes L2 for the *k* configurations the report marks as decision-relevant (those nearest the falsifier boundary and the best/worst), records divergence per quantity, and appends to the report. `acn loop promote --loop <loop_id> --provider <name>` executes L3 for one provider and appends per-provider verdicts.

**LOOP-13** The loop runner MUST treat the hypothesis file as read-only input and MUST record its hash in every bundle; if the file changes between batches, the loop MUST abort with `hypothesis_changed`.

**LOOP-14** Parameter search strategies MUST be deterministic given the loop seed; a loop report MUST be regenerable by `acn loop run --from-report <report>`.

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

## 6. Open questions (ADR candidates)

1. Whether L2 tolerance should be per-quantity in the hypothesis file (current) or a global default with per-hypothesis overrides.
2. Whether `bisect` should be allowed to propose *new* parameter axes (it should not; that is a candidate-hypothesis edit and belongs to the agent at L4-lite in lab).
3. How much of the loop report to publish with the kit: proposal is all of it, with provider raw responses redacted (TRC-42).
