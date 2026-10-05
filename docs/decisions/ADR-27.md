# ADR-27 — T06b3: a thinned verdict trajectory

**Status:** accepted (T06b3; `spec-change` to SPEC 085 v0.3, and Class C in `crates/acn-hyp`). **IDs affected:** LOOP-10, LOOP-11, LOOP-14; P4-7.

## Context
The first attempt at POC 4's L1 run of record (P4-7) wrote 240 of its 1 548 bundles in about 30 minutes. The cause was LOOP-10(c), which asked for a verdict after every batch.
- Each verdict reads every bundle so far, and on POC 4 that includes a bootstrap per grid cell, so its cost grows with the run.
- 1 536 batches therefore cost time quadratic in the grid: about 20 hours.

SPEC 100 §7 question 4 recommended thinning the trajectory.

## Decision
- **Where the verdict is computed (SPEC 085 v0.3, LOOP-10(c)).**
  - After every k-th batch, with k = ⌈budget / 50⌉, so at most fifty verdicts come before the last.
  - After the last batch, whatever k.
  - A budget of 50 or fewer keeps a verdict after every batch, so the small loops of the tests and the acceptance suites do not change.
- **Why k depends on the budget.** The budget is an input of `loop_id`, so k is fixed before the loop starts and a regeneration judges the same batches (LOOP-14). The batch count would not do: it is not known in advance for `random`, and it differs with a reused control.
- **How the report shows it (LOOP-11).** An unjudged batch records `"verdict": null` and `"reasons": null`, and `report.md` shows it as `not judged (LOOP-10(c))`. The layout keeps `acn-bench/loop-report/v1`, because no report has been cited yet.
- **`loop_run::trajectory_step(budget)`** is public, and its known answers are tested: 50 → 1, 51 → 2, 1 548 → 31.

## Consequences
- POC 4's run of record makes 50 intermediate verdicts and one final verdict instead of 1 536.
- A loop's aborts on a refused verdict (LOOP-10(f)) are detected at the next judged batch rather than at once. The bundles made in between stay, as any abort's do.
