# ADR-46 — GATE-23's loop of record, and `twin_failed` on untwinned cells

**Status:** accepted (T17.2, issue #81). **IDs affected:** GATE-23, HYP-22, HYP-24, LOOP-12, LOOP-30, P16-20, P16-21, CON-25, CON-26.

## Context
T17.2 needs a loop of record for GATE-23: a loop on the mock, twinned in `live`, whose hypothesis gives a tolerance for every quantity its predicate reads, and where every twinned quantity on its evidence page is within tolerance (ADR-42).

The first loop of `lab/hypotheses/p4-twin.toml`, `f32216f4…`, failed its twin. The cause was the form of a tolerance, not a sim/live disagreement. The maintainer gave `cached_token_ratio` an absolute tolerance on 2026-10-09. The edited candidate was looped again as `90f7fda2…` (L1 verdict `bf17daba…`) and twinned with `--top 1` (L2 verdict `8401912d…`) on build `60d0899c…` (commit `3f24936`).

The figures are in `docs/lab/2026-10-09-p4-twin-zero-base.md` ("The re-run") and on `docs/evidence/p4.md`, section "Divergence, sim against live (LOOP-12)". In short:
- every twinned quantity is within its tolerance;
- every decision cell is twinned, so there is no `partially-twinned` label;
- yet the anthropic and openai slices of the L2 verdict list `twin_failed`, on untwinned cells.

**Why those cells fail.** Controls are per workload, and the twin ran two workload controls live in each slice. Every other cell of those workloads therefore has a live control arm and no live treatment arm. Its divergence, the larger of its arms', is undefined. HYP-22 says that when `twin_required` is true, "a cell that has live bundles and whose divergence exceeds a tolerance, or is undefined, fails the twin rule". So the code fails those cells. No divergence is measured on any of them.

**What that does to the verdict.**
- At L1, anthropic and openai **pass**. L1 is `inconclusive` only because sglang and vllm, unpriced on the mock, are.
- At L2, `twin_failed` alone makes the anthropic and openai slices inconclusive, so 0 of 2 providers are reported and the guard fires.
- The twin rule, as written and coded, turned two passing providers into inconclusive ones. Under the frozen code, any `--top k` twin of a `twin_required` file will do the same.

## Decision
- **The loop of record for GATE-23 is `90f7fda2…`.**
  - Its `report.json`, its twin and both verdicts are committed under `docs/runs/` byte for byte (P16-20).
  - `docs/evidence/p4.md` is rendered from them (P16-21).
  - The failed loop `f32216f4…` stays a lab record and is not committed.
- **GATE-23 is read on twinned quantities, not on the L2 verdict.** Its text and SPEC 095's record row say "no twinned quantity is outside its tolerance". HYP-22 defines a twinned cell as one with both arms live. The `twin_failed` reasons on untwinned cells involve no sim/live divergence. On that reading GATE-23 is met by this loop. The M1 record (T17.4) cites this ADR and issue #81 in GATE-23's row, so that the reading and the verdict are both in view. The sign-off stays the maintainer's (GATE-27).
- **The code is not changed.** It follows HYP-22 as written, and `crates/acn-hyp` is in the frozen set. Issue #81 proposes a HYP-22 spec-change that judges only cells whose arms both have live bundles. That is Class C, with its own review. It would change no divergence figure, but it would change the L2 verdict's provider statuses.
- **The verdicts stay `exploratory` and `mock-gated`** and are not results (CON-26). The evidence page marks them not citable (HYP-23).

## Consequences
- **This ADR, not the evidence page, explains `twin_failed`.** The page does not render slice-level reasons. The T17.4 row cites the page's divergence section and this ADR.
- **The page's provider table is L1's.**
  - "Providers (HYP-24)" on `docs/evidence/p4.md` is rendered from the loop report's verdict, which is L1, and is not labelled with its layer. It shows anthropic and openai as pass.
  - At L2 all four are inconclusive, for the reason above.
  - A per-layer table is a follow-up renderer change (`crates/xtask`, Class A), issue #83.
- **The page is the candidate's, under the frozen file's id.**
  - The candidate keeps POC 4's `id = "p4"`, so its loop renders to `docs/evidence/p4.md`. The page says "p4 (candidate)" and marks every verdict not citable, but its title is the frozen hypothesis's.
  - When a loop of `hypotheses/p4.toml` is committed, both would render to one page. Pages keyed by hypothesis path or hash are part of issue #83.
  - The candidate's id is not changed here: that would change its hash and seed (HYP-9) and require a new loop and twin.
- **CI does not yet check GATE-23's substance.** `docs-inventory --check` checks freshness and that what a report names is committed. It does not recompute `within`, or check the candidate's file against the hash the report records (`lab/` is outside `env-hash`). Both are follow-up `xtask` checks, issue #84.
- **Verification needs the maintainer's machine.** The bundles stay under `runs/` (P16-20). To verify:
  - build at commit `3f24936` with `cargo build --release --locked -p acn-cli`, which gives build `60d0899c…`;
  - from the repository root, run `target/release/acn evidence verify --runs-dir <repo>/runs 90f7fda2ad78739ee0f8232d01b22d2327597539c9ca408c1511cded82a9542e`.

  A newer build refuses with `not_regenerable_with_this_build` (CON-31). Run this way on 2026-11-09, it returned `ok` with no findings, in about 13 minutes:
  - 1 564 bundles, 1 548 `sim` and 16 live;
  - 2 verdicts recomputed to their bytes;
  - the report regenerated.
- **The loop's `sim` bundles must be remade for T17.3.** GATE-25 asks for manifests made on one build from a clean checkout of the kit's tag. That is T17.3's work, not this ADR's.
