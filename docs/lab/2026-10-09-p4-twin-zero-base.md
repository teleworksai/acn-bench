# Lab note — p4-twin, a relative tolerance over a zero base (2026-10-09)

Candidate: `lab/hypotheses/p4-twin.toml`. Track: lab (CON-23). **Every number here is from
the mock (`mock-gated`, CON-26) and exploratory; none may be cited (CON-24).**
- Build: `3f24936`, engine `f0bba496…`.
- Machine: one macOS arm64 machine.

## Question

Does the first L2 twin of POC 4's design, run for GATE-23 (T17.2), fall within the
candidate's sim/live tolerances?

## What was tried

1. **The L1 loop.** Loop `f32216f4…` of `p4-twin.toml`, as first written, with
   `sim_live_tolerance = { cached_token_ratio = 0.05, ttft_p50_ms = 0.15, cost_per_success = 0.05 }`,
   all relative. Budget 1 548 sim bundles; it stopped `exhausted` with L1 verdict
   `c14eb1e4…` `inconclusive`. The verdict had to be inconclusive: the guard needs two
   reported providers, and the mock reports none.
2. **The twin.** `acn loop twin --top 1` ran 16 live bundles on the served mock between
   16:13 and 01:22 (about 9 hours). L2 verdict `782e3a64…`, `twin_failed: true`.
3. **The re-run (2026-10-09 to 2026-10-10).** After the decision below, loop `90f7fda2…`
   of the edited candidate (`57dbedc2…`, seed 2145617300181220395) ran the same 1 548 sim
   bundles, L1 verdict `bf17daba…` `inconclusive`, same reason as the first loop. Its
   `--top 1` twin, under `caffeinate -i`, ran 16 live bundles, the last finishing
   2026-10-10 17:17. L2 verdict `8401912d…` `inconclusive`. This is the loop of record for
   GATE-23 (T17.2, ADR-46), committed under `docs/runs/`.

## What was learned

**The twin failed on the form of a tolerance, not on a sim/live disagreement.**
- **What passed:** every measured divergence was within its tolerance: 33 of the 36
  checks (12 cells × 3 quantities). For ttft, the largest arm divergence was 0.065 of a relative 0.15. For cost, it
  was 0.0003 of a relative 0.05.
- **What failed:** `cached_token_ratio` in the three anthropic treatment cells with
  `cache_breakpoint_placement = none`.
- **Why:** on `mock-explicit`, no breakpoint means nothing is cached, so the sim mean is
  exactly 0.0. HYP-22 divides a relative divergence by `|mean_sim|` and makes a zero
  `mean_sim` undefined, and an undefined divergence fails the twin rule when
  `twin_required` is true. The code follows the spec as written (`diverge` in
  `crates/acn-hyp/src/verdict.rs`).

**The live time of a twin on the mock is hours, not minutes.**
- The served mock emits tokens on the wall clock (MLM-5, MLM-30), so one live bundle of 20
  replicate sessions takes roughly 30–60 minutes.
- A `--top 1` twin of the four-provider design ran 16 bundles.
- Plan for overnight runs. Expect local test suites with wall-clock waits to flake
  alongside one (issue #73).

**A detached twin on a laptop must hold off idle sleep.** The second twin's bundles grew
from about 25 minutes to 11 hours overnight.
- **Where the time went:** the 11-hour bundle's own spans cover 0.72 h, back to back,
  with the same 1 029 calls as its sim twin and per-call times close to sim.
- **Why:** `pmset -g log` shows the machine idle-sleeping after a minute, with only
  10-second DarkWakes. The monotonic clock the trace reads stops during sleep, so the
  recorded timings stayed sound while the wall time ballooned.
- **What it is not:** a fresh repro of that cell, run while the machine was awake, ran at
  sim speed. So it is not a harness or mock defect.
- **The fix:** the twin was stopped and restarted under `caffeinate -i`.

## Decision

The maintainer chose on 2026-10-09 to give `cached_token_ratio` an absolute tolerance in
the candidate, `{ abs = 0.01 }`.
- A ratio in [0, 1] that is legitimately 0 in whole regions of the grid has no meaningful
  relative scale.
- The other two quantities stay relative.

Two alternatives were set aside:
- a HYP-22 spec-change that treats exact agreement as divergence 0 (Class C);
- recording the failure and leaving GATE-23 open.

The loop and twin are re-run on the edited candidate, which has a new hash and therefore
a new seed (HYP-9). The failed run is kept here as a record and is not committed under
`docs/runs/`.

## The re-run

**Every twinned quantity is within its tolerance: 36 of 36 checks** (12 twinned cells × 3
quantities, 10 of them decision cells, all twinned, so no `partially-twinned` label).

| Quantity | Tolerance | Largest twinned divergence |
|---|---|---|
| `cached_token_ratio` | abs 0.01 | 0.00008 |
| `ttft_p50_ms` | relative 0.15 | 0.056 |
| `cost_per_success` | relative 0.05 | 0.00024 |

**The L2 verdict still lists `twin_failed`, on cells that were never twinned.**
- In the anthropic and openai slices, 253 and 247 cells carry it.
- Each is an untwinned, non-decision cell. Its control arm is the slice's control, which
  the twin ran live; its treatment arm has no live bundle.
- Its divergence is therefore undefined. HYP-22 fails "a cell that has live bundles and
  whose divergence … is undefined", and the code reads a shared control arm as the cell
  having live bundles.
- No measured divergence is involved, and GATE-23 reads twinned quantities only, so it is
  met (ADR-46). The wording is raised as issue #81 for a later HYP-22 spec-change.

## Graduate / park / drop

- **The absolute tolerance for ratio quantities: park.** If frozen files meet the same
  zero base, a HYP-22 spec-change, treating identical means as divergence 0, is the
  general fix. Propose it then, not now.
- **`twin_failed` on untwinned cells that share a live control: park,** with issue #81. A
  HYP-22 spec-change that judges only cells whose arms both have live bundles is Class C
  and changes no number on the M1 page.
