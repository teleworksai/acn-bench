# SPEC 100 — POC 4: harness cache discipline as a controlled variable

**Status:** Draft v0.2 (October 2026; v0.2, with T06b: the P4-8(c) fixture's name follows HYP-2, backfilled results stay in context, `retrieval` gets a cacheable tool block, and P4-12 holds the run of record until the mock's compaction and fan-out replies are not degenerate). **Inherits:** SPEC 000, 010, 030, 040, 080, 085. **Prefix:** P4. **Hypothesis:** `hypotheses/p4.toml` (frozen). **Crates:** none of its own; it is run by `acn-harness` (SPEC 040), `acn-hyp` (SPEC 080, 085) and `acn-cli`, with its suite in `tests/accept/p4.rs`.
**Purpose:** define the protocol that turns `hypotheses/p4.toml` into a verdict per provider:
- the three workloads it varies over;
- the providers, and what each runs on the mock;
- the run of record on the mock, and the acceptance suite that gates it;
- what must be settled and recorded before a live run on real providers can give a citable verdict.

## 0. The question

The report's claim (§3.6, Appendix E POC 4) is that the harness, not the model or the serving layer, decides cacheability, and that its discipline moves cost per successful task by more than the run-to-run noise floor. `hypotheses/p4.toml` states this as a falsifier: the hypothesis fails when no knob configuration moves `cost_per_success` by more than the control's 95% noise floor.

**Per provider.** Real providers differ in mechanics: explicit breakpoints (Anthropic `cache_control`), automatic prefix matching (OpenAI), and block-granular prefix caching (vLLM, SGLang). The hypothesis is therefore instantiated per provider (CON-26).

**What the mock can show.** The mock reproduces whatever cache rules it is given. A mock verdict therefore tests the harness against our own model of caching. It gates the suite and is never cited (PLAN §M0).

**The two slices are not equally informative.** On an explicit-breakpoint provider, the placement `none` turns caching off altogether, so that slice is close to certain to show an effect. The informative slices are those of providers that cache automatically.

## 1. Definitions

- **Stable prefix** of a task:
  - It is ⌈b / 4⌉ (MLM-11's estimate), where b is the bytes of the system messages of the task's first request, sent with every knob at its control value except `timestamp_in_system_prompt`, which is `false`. The bytes are MLM-10's prompt bytes.
  - The isolation marker (HAR-42) is subtracted wherever it appears.
  - A workload's stable prefix is the minimum over its tasks.
- **Grid** — every cell of `hypotheses/p4.toml`: 2⁵ × 4 = 128 knob configurations × 3 workloads, per provider. That is 384 treatment cells and 3 controls per provider, since `workload` is pooled.
- **Success** — a turn whose outcome is `success` under the checker (HAR-3). P4 workloads state no `expect_tools` (P4-3), so a turn succeeds when it ends, within its call limit, with a reply that is not a tool call. `cost_per_success` on POC 4 is therefore the cost per completed turn.

## 2. Workloads

**P4-1** The `workload` values of `hypotheses/p4.toml` MUST be these three files, each a workload of HAR-60:
- `coding` is `workloads/p4-coding.toml`;
- `retrieval` is `workloads/p4-retrieval.toml`;
- `fanout` is `workloads/p4-fanout.toml`.

They are given to `acn loop run` as a map (`--workload coding=workloads/p4-coding.toml …`, LOOP-10).

**P4-2** Every P4 workload MUST have a stable prefix of at least 2 048 estimated tokens. That is above the mock profiles' minimum cacheable prefix of 1 024 tokens (MLM-50), so caching engages on the mock. A model pinned for a live run (P4-9) MUST have a documented minimum cacheable prefix no larger than half of each workload's stable prefix, so that the estimate's distance from the provider's tokenizer cannot leave caching off.

**P4-3** Each P4 workload MUST have all of the following.
- **Tasks:** at least two tasks of at least three turns each. Each tool of the workload is the first tool of some task, since the mock's reply policy calls the first tool of the task's list (MLM-40).
- **Tools:** at least three. Every `result_bytes` range has `max > min`, so that replicates differ (the noise floor is not degenerate).
- **No `expect_tools`** on any turn. Otherwise `tool_order_stable = false` would move success through the checker, not through caching (ADR-17).
- **`max_tokens`** of at least 256 for the agent and the summary, enough for a tool call and a short reply on every pinned model.

The workloads differ as follows:
- **`coding`** edits a repository. It has `read_file` and `grep` tools and at least one turn with `updates` (HAR-13). Every task compacts at least once under `window_full`, at the control's other knob values on `mock-explicit` and `mock-auto`, but never at or before its turn with `updates`, so the backfilled result is still in context. Every task also compacts at least once under `read_cost_threshold` with `timestamp_in_system_prompt = true`, on both profiles.
- **`retrieval`** answers questions from long tool results. It has a tool whose `result_bytes` minimum is 4 000, at least one turn with `updates`, and a tool block (the tool definitions as JSON) of at least 1 024 estimated tokens, so that a `system_and_tools` breakpoint (HAR-16) has something to cache above the minimum.
- **`fanout`** delegates. It has a `subagent` tool of width at least 3.

Taken together, the three workloads MUST give every value of every knob of HAR-11 to HAR-16 a workload in which it changes the requests. This coverage rule is SPEC 100's own: a knob inert on every workload would measure nothing.

**P4-4** The workload files are hashed inputs (CON-27(a)). When `hypotheses/p4.toml` is pinned (HYP-9, HYP-26), `pins.workload` MUST list exactly their three hashes. A change to a workload file after the first cited run gives it a new hash, and so a new run under a new pin; a cited run is never edited.

## 3. Providers and layers

**P4-5** The providers are the `provider` values of `hypotheses/p4.toml`. A verdict is conclusive only when at least two providers are reported (the file's guard and `min_providers_for_verdict`, HYP-24): `anthropic` and `openai` at M0, `vllm` and `sglang` once a node exists (M3).
- **On the mock (L1, LOOP-10)** each provider MUST run on the profile that models its mechanics (MLM-50): `anthropic=mock-explicit`, `openai=mock-auto`, `vllm=mock-blocks`, `sglang=mock-blocks`.
- **Prices.** `cost_per_success` has a price row for `anthropic` and `openai` only (`PRICES` in `crates/acn-hyp/src/quantities.rs`, ADR-19). The `vllm` and `sglang` slices are therefore inconclusive until a Class C change adds rows for them.

**P4-6** *(informative)* `hypotheses/p4.toml` declares `twin_required = false`. POC 4 is network-free: the harness calls its endpoint directly (ADR-17), and there is no link to twin. The L2 layer is therefore skipped, and the L3 gate (LOOP-4) is waived. The waiver is reviewed, not machine-checked (ADR-24).

**P4-7** The L1 run of record MUST be `acn loop run --hypothesis hypotheses/p4.toml` over the whole grid. It uses:
- the workload map of P4-1 and the model map of P4-5;
- a budget equal to the grid's bundle count: 4 providers × 387 = 1 548 (LOOP-10(e));
- a binary built with `--release`.

It is made only once P4-12 holds. Its loop report, its final verdict and the `acn evidence verify` result make up the M0 mock deliverable: a *model-of-caching* table, labelled `mock-gated` (HYP-23), never cited.
- **The file-level verdict is inconclusive by construction**, because the `vllm` and `sglang` slices are unpriced (P4-5, HYP-24).
- **The deliverable is therefore the per-slice `anthropic` and `openai` verdicts, with each cell's effect (CON-18).**

**P4-12** The run of record (P4-7) MUST NOT be made, and no live run (P4-9) proposed, until the mock's replies are not degenerate for POC 4. Three conditions must hold:
- **(a) Compaction summaries are text.** Every compaction call (HAR-4) gets a text reply, not a tool call. Today the compaction call carries the tools, and the mock answers it with a tool call (MLM-40), so the summary is empty and compaction only deletes history.
- **(b) Forked children run their own tools.** Under `fork_from_prefix` (HAR-14), children call the tools of their child specification. Today they inherit the parent's tool list and call the `subagent` tool, which fails.
- **(c) The tool-order confound is resolved or recorded.** The effect of `tool_order_stable` either does not change which tools the mock calls, or that confound is stated with the knob's effect.

The SPEC 040 and SPEC 030 changes that settle these conditions are T06b2's (issues #24, #25 and #26).

## 4. The acceptance suite

**P4-8** `tests/accept/p4.rs` MUST run on the mock in CI and MUST check:
- **(a)** Each P4 workload loads (HAR-60) and meets P4-2 and P4-3.
- **(b)** Each of the six knobs changes the requests of at least one P4 workload on the `mockllm` backend, on one replicate per knob value.
- **(c)** A reduced L1 loop over a checked-in candidate, `tests/accept/fixtures/p4t-timestamp.toml` (named `<id>-<slug>`, HYP-2). The candidate:
  - has id `p4t`;
  - varies `timestamp_in_system_prompt` and `provider` (`anthropic`, `openai`), with `[control].config = { timestamp_in_system_prompt = true }`;
  - otherwise takes `hypotheses/p4.toml`'s measures, replicates, guard and falsifier.

  It runs on the `coding` workload with the P4-5 profiles. The loop MUST complete, and its report MUST regenerate (LOOP-14). Its final verdict MUST:
  - carry `mock-gated` and `exploratory`;
  - judge each provider in its own slice, each `pass`;
  - give the `timestamp_in_system_prompt = false` cell a `cost_per_success` effect whose 95% interval lies below zero on both providers.

  This shows that the mock's model of caching reacts to a timestamp in the system prompt as built. It is not a POC 4 result.
- **(d)** The control. The control bundle of each slice in (c) is the harness's shipped default configuration (HAR-10). The loop report gives each cell's treatment-minus-control effect, with its replicate count and 95% interval (CON-18).

The full grid of P4-7 is too long for CI. It runs on demand and in the nightly tier.

## 5. Live runs (L3)

**P4-9** A live POC 4 verdict MUST be made of real-provider bundles only (HYP-20 refuses mixing mock and real), for at least two providers. It MUST be computed by `acn hyp verdict`, with every grid cell present at `[design].replicates` replicates for each provider (HYP-21). It MUST NOT be cited until all of the following hold:
1. **The falsifier is settled.** A `spec-change` settles SPEC 080 §6 question 1:
   - whether `max_over_knobs` over 384 cells needs a simultaneous interval or a null distribution of the maximum;
   - whether knobs inert on a provider (the four breakpoint placements on automatic-caching providers, HAR-16) need conditional domains.

   A `p4` successor that `supersedes` this file applies the answer.
2. **The file is pinned (HYP-26).** Its pins are its workloads (P4-4), scenario hash zero (ADR-17), and one model per provider whose minimum meets P4-2. Each model's prices MUST match its row's `reference` in `PRICES`, or a Class C change updates the row. Pinning changes the file's hash, and so its seed (HYP-9), so the L1 run of record MUST be redone on the pinned file. Pins and mock runs conflict until SPEC 080 §6 is changed as §7 question 3 says.
3. **The maintainer approves** the provider models, the request each model accepts, and the spend estimate of P4-10. The request covers the parameters a model refuses (some models refuse `temperature`, which the harness always sends today), a `max_tokens` large enough for reasoning tokens as well as the reply, and the treatment of a reply cut by its token limit (`finish_reason = "length"`), which HAR-3 counts as a success today (issue #27).
4. **One build runs the whole campaign** (HYP-20 refuses more than one build per mode). The campaign's start and end times are recorded with its verdict.

**P4-10** The spend estimate of a live run MUST be computed from the L1 run of record on the pinned file, and stated in the PR that proposes the live run. It is the sum, over the provider's grid bundles, of the calls' tokens weighted by that provider's list prices. Its shape is 387 bundles per provider at 20 replicates, which is 7 740 workload replicates per provider, each of a few tens of calls. On a provider that caches automatically, 288 of the 384 cells repeat another cell's requests, because breakpoints are not sent there (HAR-16). The estimate MUST state that share.

**P4-11** Each live bundle MUST record three things:
- its endpoint host (CON-26);
- the provider's usage on every call, normalised by TRC-21 (HAR-31);
- the execution order derived from the seed (CON-5(d)).

A provider whose responses do not report cache reads makes its `cost_per_success` undefined, and its slice inconclusive (HYP-11).

## 6. Acceptance tests

- `tests/accept/p4.rs` — P4-1, P4-2, P4-3, P4-5, P4-8:
  - P4-8 as listed;
  - P4-7's arguments accepted at budget 1 548 and refused at 1 547.
- `crates/acn-hyp/tests/existing_files.rs` — P4-4, P4-5: `hypotheses/p4.toml` loads with the provider and workload values P4-1 and P4-5 name, and a `pins.workload`, once present, equals the three workloads' hashes.
- P4-9 to P4-11 are the live tier's (T06d). Their tests are `#[ignore]` tests, run with `--features real-api`.

## 7. Open questions (ADR candidates)

1. **Live runs.** LOOP-12's `acn loop promote` is scheduled with T30, which is M3 and hardware-gated. Hosted providers need no hardware, and T06 needs live runs at M0. Recommendation: a SPEC 085 change that lands `loop promote` for hosted providers as T06c, ahead of T30's node adapters.
2. **Live cost and the design.** The frozen file's grid makes 384 cells per provider mandatory for a decided verdict. A smaller design cannot be expressed in SPEC 080, which knows only `grid`; one knob at a time from the control would be 8 cells per workload, and would drop "in combination", so it is a new hypothesis. Conditional domains (P4-9 item 1) alone cut an automatically caching provider's grid by 4×. Recommendation: settle P4-9 item 1 first, then price the resulting grid from the L1 run (P4-10).
3. **Pins and the mock.** HYP-20 checks `pins.models` on every bundle, mock bundles included. A pinned `p4.toml` therefore refuses its own mock runs: a provider's model cannot be both the mock profile and the real model. Recommendation: apply `pins.models` to real-provider bundles only in SPEC 080, since mock bundles are never cited (CON-26). Filed with issue #18; until then `p4.toml` stays unpinned, and its verdicts carry `unpinned-inputs` (HYP-23).
4. **Trajectory cost.** LOOP-10(c) computes a verdict after every batch, so the L1 run of record spends most of its time on intermediate verdicts: 1 536 batches over up to 1 548 bundles. Recommendation: a SPEC 085 change that computes the trajectory at most every ⌈n/50⌉ batches, n being the grid's batch count, and at the last batch.
5. **Live order and provider drift.** The loop runs cells in grid order, and a slice's control runs with its first batch. Over a campaign of hours or days, provider drift is confounded with the knob effects. Recommendation: at L3, run cells in a seeded shuffle and repeat the control bundle at intervals, through a SPEC 085 change with T06c.
6. **OpenAI routing.** Without a `prompt_cache_key`, OpenAI cache hits depend on routing, which raises the noise floor (SPEC 040 §10 question 2). Recommendation: settle with T06c, before the openai pin.
