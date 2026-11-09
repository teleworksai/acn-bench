# ADR-46 — GATE-23's loop of record, and `twin_failed` on untwinned cells

**Status:** accepted (T17.2, issue #81). **IDs affected:** GATE-23, HYP-22, LOOP-12, P16-20, P16-21, CON-25, CON-26.

## Context
T17.2 needs a loop of record for GATE-23: a loop on the mock, twinned in `live`, whose hypothesis gives a tolerance for every quantity its predicate reads, and where every twinned quantity on its evidence page is within tolerance (ADR-42).

The first loop of `lab/hypotheses/p4-twin.toml`, `f32216f4…`, failed its twin. The cause was the form of a tolerance, not a sim/live disagreement. The maintainer gave `cached_token_ratio` an absolute tolerance on 2026-10-09 (`docs/lab/2026-10-09-p4-twin-zero-base.md`). The edited candidate was looped again as `90f7fda2…` (L1 verdict `bf17daba…`) and twinned with `--top 1` (L2 verdict `8401912d…`) on build `60d0899c…` (commit `3f24936`).

On the re-run's L2 verdict:
- **Every twinned quantity is within its tolerance.** There are 12 twinned cells × 3 quantities, with no `within: false` on the evidence page.
- **Every decision cell is twinned,** 2 in the anthropic slice and 8 in openai, so the verdict carries no `partially-twinned` label.
- **The anthropic and openai slices still list `twin_failed`,** on 253 and 247 cells. Each of those is an untwinned, non-decision cell whose control arm is the slice's shared control, which was run live, and whose treatment arm has no live bundle.

HYP-22 says that when `twin_required` is true, "a cell that has live bundles and whose divergence exceeds a tolerance, or is undefined, fails the twin rule". With one arm live, the cell's divergence, the larger of its arms', is undefined, so the code fails it. No divergence is measured on those cells at all.

## Decision
- **The loop of record for GATE-23 is `90f7fda2…`.** Its `report.json`, its twin and both verdicts are committed under `docs/runs/` byte for byte (P16-20), and `docs/evidence/p4.md` is rendered from them (P16-21). The failed loop `f32216f4…` stays a lab record and is not committed.
- **GATE-23 is read on twinned quantities,** as its text and SPEC 095's record row say ("no twinned quantity is outside its tolerance"). The `twin_failed` reasons on untwinned cells do not block it, because no sim/live divergence is involved. GATE-23 is therefore met by this loop.
- **The code is not changed.** It follows HYP-22 as written, and `crates/acn-hyp` is in the frozen set. The wording is raised as issue #81 for a later HYP-22 spec-change, which would judge only cells whose arms both have live bundles. That change is Class C, with its own review.
- **The verdicts stay `exploratory` and `mock-gated`** and are not results (CON-26). The evidence page marks them not citable (HYP-23).

## Consequences
- The M1 record (T17.4) cites `docs/evidence/p4.md` and this ADR for GATE-23.
- **A reader of the L2 verdict sees `twin_failed` in two slices.** The evidence page's divergence table, where every row is `within: true`, and this ADR are where that is explained.
- **Bundles stay under `runs/` on the maintainer's machine.** `acn evidence verify 90f7fda2…` needs a binary of build `60d0899c…`, built from commit `3f24936` (CON-31). A newer build refuses with `not_regenerable_with_this_build`. Run with that build on 2026-11-09, it returned `ok` with no findings: 1 564 bundles (1 548 `sim`, 16 live), 2 verdicts recomputed to their bytes, and the report regenerated.
- **The loop's `sim` bundles must be remade for T17.3.** GATE-25 asks for manifests made on one build from a clean checkout of the kit's tag. That is T17.3's work, not this ADR's.
