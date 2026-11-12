# ADR-54 — SPEC 080 v0.3 in the verdict crate: three readings made while writing it

**Status:** accepted (`env-change`; issues #10, #13, #15, #18, #81; follows ADR-50). **IDs affected:** HYP-9, HYP-20, HYP-22, HYP-27, HYP-28; by reference HYP-15, HYP-24, LOOP-10, CON-7.

## Context
ADR-50 settled five issues in the text of SPEC 080 v0.3 and left the verdict crate behind the spec until gate M1 was signed. The gate closed on 2026-11-12. This PR brings the code to the spec. Three points of the text needed a reading before they could be written as code.

## Decision
**What the code now does, as v0.3 says.**
- **HYP-22.** A cell fails the twin rule only when it is twinned: both arms live, with at least half the replicates paired. The figures of a cell that is not twinned are still computed and recorded. A relative divergence whose numerator is exactly zero is zero, whatever its base.
- **HYP-9, HYP-20.** In a file with no `provider` parameter, `pins.models` has exactly one key, a backend name other than `mockllm`. The model pin is checked on real-backend bundles only, under the bundle's provider value or, with no `provider` parameter, its backend. The scenario and workload pins apply to every bundle.
- **HYP-27.** Lint makes one witness search. A quantity term and a `noise_floor` are probed with the non-negative values, an interval bound with those and their negatives. A witness that needs a negative bound is no longer a warning.
- **HYP-28** needed no change: a cell's `twinned` flag was already written for every cell.
- **HYP-15 and HYP-24** needed none either: the code already read them as v0.3 does (ADR-19, ADR-20).

**The readings.**
1. **A file about the mock alone pins no model.** HYP-9 says a file with no `provider` parameter has exactly one key in `pins.models`, a backend other than `mockllm`. A file whose `backends` do not include `real-api` has no such backend. It may pin its scenario and workload, and its `models` table must be empty; a key there is refused, because nothing would ever be checked against it. **This reading is against the letter of HYP-9**, whose MUST makes no exception for such a file. It is therefore also raised as `spec-conflict: HYP-9` (issue #102, CON-13), which proposes the one sentence that would make the text say this. Only mock-gated files are concerned, and none of their verdicts can be cited (CON-26).
2. **The loop's check before it runs no longer looks at `pins.models`.** LOOP-10(e) refuses a loop whose runs the pins would refuse. A loop runs on the mock, whose model is not pinned, so there is no model to check; the workload and scenario pins are checked as before.
3. **The witness search is bounded by assignments, not only by terms.** A bound has seven probe values and every other term four, and an interval the predicate reads is two bounds, its low and its high, whichever of them is named: 49 assignments. The search is made when the predicate has at most ten terms and at most 2^20 assignments, which is what ten unsigned terms come to. The limit is a product: with one interval the search is made up to seven other terms, with two up to four, with three up to one, and with four intervals never. **This is narrower than v0.2 in one place**: a predicate of one interval and eight other terms was searched by the old first pass, which gave a bound four values, and is not searched now. A predicate that is not searched is reported as such, an error in a frozen file, so the limit refuses a file and never passes one. No file in the repository is near either limit (the largest reads three terms and no interval). `tests/accept/hyp_lint.rs` holds the limit at its edges.

## Consequences
- **`engine_hash` changes**, from `f0bba496…` to the one `env-hash.json` now records. Bundles made under the old engine are refused by the new verdict tool (HYP-20), and their `run_id`s are of the old engine (CON-29). The kit at `kit-m1` and GATE-23's loop of record are of the old engine and stay reproducible at that tag.
- **A twin made from now on does not mark untwinned cells `twin_failed`.** On a loop shaped like GATE-23's, the L2 status of a provider whose control was twinned would be `pass` where it was `inconclusive`. The committed verdicts are not re-judged (ADR-46, ADR-50).
- **A real-provider file without a `provider` parameter can now be pinned and judged**, which no file could before (issue #18).
- **A file pinned to real models can still be looped and twinned on the mock**, which SPEC 100 §7 question 3 needed before `hypotheses/p4.toml` could be pinned.
- **Tests that asserted the v0.2 rule are changed**, as ADR-50 listed: two in `crates/acn-hyp/tests/verdict_semantics.rs`, one in `crates/acn-hyp/tests/loop_gating.rs`. Two more changed that it did not list: a pinned mock-model case in `crates/acn-hyp/tests/loop_run.rs`, which now runs, and a pinned file in `crates/xtask/tests/freeze.rs`, which now names its one real backend.
- **Issues #10, #13, #15, #18 and #81 close with this PR.**
