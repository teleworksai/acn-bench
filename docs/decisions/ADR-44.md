# ADR-44 — `tensormesh` joins POC 4

**Status:** accepted (issue #70: spec-change and env-change). **IDs affected:** P4-5, P4-7 (and SPEC 100 §8's test list).

## Context
The v1.5 patch added `tensormesh` to the `provider` values of `hypotheses/p4.toml`, as an OpenAI-compatible provider whose cached input tokens are billed at $0. The file has been frozen since M0 (PR 36), and SPEC 100 pinned the providers:
- P4-5 gives a mock profile for each;
- P4-5 names the priced ones;
- P4-7 sets the run of record's budget at 4 × 387.

The change was split off PR 69 and raised as `spec-conflict` issue #70. The maintainer decided on 2026-10-08 that `tensormesh` joins POC 4.

## Decision
- **The provider list.** `hypotheses/p4.toml` gains `tensormesh` among its `provider` values, the patch's hunk exactly.
- **The mock profile.** On the mock, `tensormesh` runs on `mock-auto` (P4-5): it is OpenAI-compatible, and automatic prefix caching is the mechanics `mock-auto` models. That mechanics is an assumption from "OpenAI-compatible" and the patch, not from a provider document; it MUST be checked before `tensormesh`'s profile is pinned (P4-9).
- **Live.** `tensormesh` has no harness backend: it runs live on the `openai` wire with its endpoint (P4-5), and whether that endpoint enforces `allowed_tools` (HAR-14) is unverified.
- **The budget.** The run of record's budget is 5 × 387 = 1 935 bundles (P4-7). The acceptance suite checks 1 935 and 1 934.
- **No price row yet.** `cost_per_success` needs relative prices for input, cache read, cache write and output (ADR-19). The patch gives only "cached input billed at $0". So the `tensormesh` slice stays unpriced and inconclusive, like `vllm` and `sglang`, until the maintainer supplies the rest. A row is then a Class C change to `PRICES` in `acn-hyp`.
- **The frozen tests follow.** `crates/acn-hyp/tests/existing_files.rs` pins p4's new BLAKE3 and five providers, and its fixture copy of `p4.toml` is the new file.

## Consequences
- **The frozen set changes** in `hypotheses/` and `crates/acn-hyp/`, so `env-hash.json` is rewritten. `engine_hash` moves, because the `acn-hyp` tests are under `crates/` (CON-28), and every `run_id` moves with it.
- **The M0 run of record stays as it was.** `docs/runs/2026-10-05-p4-l1-mock.md` describes a four-provider run of the earlier file, and it remains a record of that run. Under v0.3 it no longer satisfies P4-7 as written: P4-7 is unmet at HEAD until a 1 935-bundle L1 run is made.
- **On the mock the fifth slice costs and tells nothing new.** `tensormesh` runs on the same profile as `openai` and has no price row, so the run of record grows by a quarter (387 bundles; trajectory step 39, was 31) with no extra verdict until it is priced or run live.
- **The T17.2 twin candidate** (`lab/hypotheses/p4-twin.toml`, in its own PR) keeps POC 4's four providers of when it was made. It is twinned on the mock, and a fifth unpriced provider would add bundles but no verdict.
- **GATE-14 stays deferred to M2 (ADR-42).** When it runs, `tensormesh` is a third live candidate beside `anthropic` and `openai`.
