# SPEC 100 — POC 4: harness cache discipline as a controlled variable

**Status:** Draft v0.1 (October 2026). **Inherits:** SPEC 000, 010, 030, 040, 080, 085. **Prefix:** P4. **Hypothesis:** `hypotheses/p4.toml` (frozen). **Crates:** none of its own; it is run by `acn-harness` (SPEC 040), `acn-hyp` (SPEC 080, 085) and `acn-cli`, with its suite in `tests/accept/p4.rs`.
**Purpose:** define the protocol that turns `hypotheses/p4.toml` into a verdict per provider: the three workloads it varies over, the providers and what each runs on the mock, the order of the layers, the acceptance suite that gates the run on the mock, and what a live run on real providers must record before its verdict can be cited.

## 0. The question

The report's claim (§3.6, Appendix E POC 4) is that the harness, not the model or the serving layer, decides cacheability, and that its discipline moves cost per successful task by more than the run-to-run noise floor. `hypotheses/p4.toml` states it as a falsifier: the hypothesis fails when no knob, alone or in combination, moves `cost_per_success` by more than the control's 95% noise floor. Real providers differ in mechanics: explicit breakpoints (Anthropic `cache_control`), automatic prefix matching (OpenAI), and block-granular prefix caching (vLLM, SGLang). The hypothesis is therefore instantiated per provider (CON-26). The mock reproduces whatever cache rules it is given, so a mock verdict tests the harness against our own model of caching. It gates the suite and is never cited (PLAN §M0).

## 1. Definitions

- **Stable prefix** — the part of a workload's first request that no knob changes while the knobs are at their control values: the system prompt (without the isolation marker and the timestamp line, HAR-11, HAR-42) and the tool definitions, measured by the harness's length estimate, ⌈canonical bytes / 4⌉ (HAR-15).
- **Grid** — every cell of `hypotheses/p4.toml`: 2⁵ × 4 = 128 knob configurations × 3 workloads, per provider, so 384 treatment cells and 3 controls per provider.

## 2. Workloads

**P4-1** The `workload` values of `hypotheses/p4.toml` MUST be the files `workloads/p4-coding.toml` (`coding`), `workloads/p4-retrieval.toml` (`retrieval`) and `workloads/p4-fanout.toml` (`fanout`), each a workload of HAR-60, given to `acn loop run` as a map (`--workload coding=workloads/p4-coding.toml …`, LOOP-10).

**P4-2** Every P4 workload MUST have a stable prefix of at least 2 048 estimated tokens, above every provider's minimum cacheable prefix (1 024 to 2 048 tokens), so that caching can engage on every provider. A workload below that floor measures nothing: its cache counters stay at zero whatever the knobs do.

**P4-3** Each workload MUST have at least two tasks of at least three turns each, and at least three tools.
- **`coding`** edits a repository: `read_file` and `grep` results, at least one turn with `updates` (HAR-13), and a `compact_at_tokens` that every task reaches at least once under both compaction triggers (HAR-15).
- **`retrieval`** answers questions from long tool results: at least one tool whose `result_bytes` minimum is 4 000, and at least one turn with `updates`.
- **`fanout`** delegates: a `subagent` tool of width at least 3 whose child shares the parent's tools (HAR-14).

Across the three workloads, every clause of HAR-11 to HAR-16 MUST apply in at least one workload, so that no knob is inert on the whole set (HAR-17).

**P4-4** The workload files are hashed inputs (CON-27(a)). When `hypotheses/p4.toml` is pinned (HYP-9, HYP-26), `pins.workload` MUST list exactly the three hashes. A change to a workload file after the first cited run is a new hash, and therefore a new run under a new pin, never an edit of a cited one.

## 3. Providers and layers

**P4-5** The providers MUST be the `provider` values of `hypotheses/p4.toml`. A verdict MUST report at least two providers (its guard, HYP-24): `anthropic` and `openai` at M0, `vllm` and `sglang` once a node exists (M3).
- **On the mock (L1, LOOP-10)** each provider runs on the profile that models its mechanics (MLM-50): `anthropic=mock-explicit`, `openai=mock-auto`, `vllm=mock-blocks`, `sglang=mock-blocks`.
- **Prices.** `cost_per_success` has a price row for `anthropic` and `openai` only (HYP-12), so the `vllm` and `sglang` slices are inconclusive until a Class C change adds rows for them.

**P4-6** `hypotheses/p4.toml` declares `twin_required = false`: POC 4 is network-free, the harness calls its endpoint directly (ADR-17), and there is no link to twin. The L2 layer is therefore skipped, and the L3 gate (LOOP-4) is waived. The waiver is reviewed, not machine-checked (ADR-24).

**P4-7** The L1 run of record MUST be `acn loop run --hypothesis hypotheses/p4.toml` over the whole grid, with the workload map of P4-1, the model map of P4-5, and a budget equal to the grid's bundle count (4 providers × 387 = 1 548, LOOP-10(e)), built with `--release`. Its loop report, final verdict and `acn evidence verify` result are the M0 mock deliverable: a *model-of-caching* table, labelled `mock-gated` (HYP-23), never cited.

## 4. The acceptance suite

**P4-8** `tests/accept/p4.rs` MUST run on the mock in CI and MUST check:
- **(a)** each P4 workload loads (HAR-60) and meets P4-2 and P4-3;
- **(b)** each of the six knobs changes the requests of at least one P4 workload, and changes nothing where its clause does not apply (HAR-17), on one replicate per knob value;
- **(c)** a reduced L1 loop: a candidate derived from `hypotheses/p4.toml` that varies `timestamp_in_system_prompt` only, over `anthropic` and `openai` on their mock profiles and the `coding` workload, with the frozen file's replicates, guard and falsifier. It MUST complete, its report MUST regenerate (LOOP-14), and its final verdict MUST:
  - carry `mock-gated` and `exploratory`;
  - judge each provider in its own slice;
  - show the mechanism the mock models: turning the timestamp off lowers `cost_per_success` on both providers by more than the control's noise floor.

  (c) is a check that the mock's model of caching reacts as built. It is not a POC 4 result.
- **(d)** the control: the control bundle of each slice in (c) is the harness's shipped default configuration (HAR-10, CON-18).

The full grid of P4-7 is too long for CI; it runs on demand (P4-7) and in the nightly tier.

## 5. Live runs (L3)

**P4-9** A live POC 4 verdict MUST be made of real-provider bundles only (HYP-20 refuses mixing mock and real) for at least two providers. The verdict MUST be computed by `acn hyp verdict`. Each provider MUST have every grid cell, each with `[design].replicates` replicates, so that the frozen file's grid is complete (HYP-21). Before the first live run:
- `hypotheses/p4.toml` MUST be pinned (HYP-26): its workloads (P4-4), scenario hash zero (ADR-17), and one model per provider.
- The maintainer MUST approve the provider models and the spend estimate of P4-10.

**P4-10** The spend estimate of a live run MUST be computed from the L1 run of record and stated in the PR that proposes the live run. It is the sum over the provider's grid bundles of the calls' tokens, weighted by that provider's list prices. Shape of the cost: 387 bundles per provider at 20 replicates is 7 740 workload replicates per provider, each of a few tens of calls.

**P4-11** Each live bundle MUST record its endpoint host (CON-26), the provider's cache counters on every call (HAR-31), and the execution order derived from the seed (CON-5(d)). A provider whose responses do not report cache reads makes its `cost_per_success` undefined, and its slice inconclusive (HYP-11).

## 6. Acceptance tests

- `tests/accept/p4.rs` — P4-1, P4-2, P4-3, P4-5, P4-8: as P4-8 lists.
- `crates/acn-hyp/tests/existing_files.rs` — P4-5, HYP-16: `hypotheses/p4.toml` loads with the provider and workload values P4-1 and P4-5 name.

## 7. Open questions (ADR candidates)

1. **How live runs are driven.** LOOP-12's `acn loop promote` is scheduled with T30 (M3, hardware-gated), but hosted providers need no hardware and T06 needs live runs at M0. Recommendation: implement `loop promote` for hosted providers as T06c, ahead of T30's node adapters.
2. **Live cost of the full grid.** The frozen file's grid makes 384 cells per provider mandatory for a decided verdict. A smaller design (one knob at a time from the control, 8 cells per workload) cannot be expressed in SPEC 080, which knows only `grid`. Recommendation: price the full grid from the L1 run of record (P4-10) before deciding. If it is too costly, propose a `oat` (one-at-a-time) design kind in SPEC 080 and a `p4` successor that `supersedes` it.
3. **Pins and the mock.** HYP-20 checks `pins.models` on every bundle, mock bundles included. A pinned `p4.toml` therefore refuses its own mock runs, because a provider's model cannot be both the mock profile and the real model. Recommendation: in SPEC 080, apply `pins.models` to real-provider bundles only, since mock bundles are never cited (CON-26). Filed with issue #18; until it is resolved, `p4.toml` stays unpinned, and its verdicts carry `unpinned-inputs` (HYP-23).
4. **Trajectory cost.** LOOP-10(c) computes a verdict after every batch, so the L1 run of record (1 536 batches over up to 1 548 bundles) spends most of its time on intermediate verdicts. Recommendation: a SPEC 085 change that computes the trajectory at most every ⌈n/50⌉ batches, n being the grid's batch count, as well as at the last batch.
