# ADR-50 — SPEC 080 v0.3: five open conflicts settled in one amendment

**Status:** accepted (`spec-change`; issues #10, #13, #15, #18, #81). **IDs affected:** HYP-9, HYP-10, HYP-15, HYP-20, HYP-22, HYP-24, HYP-27, HYP-28; SPEC 100 §7 question 3; by reference HYP-13, HYP-21, HYP-23, CON-25, CON-26, LOOP-12, GATE-23.

## Context
SPEC 080 had five open issues against it. Four are `spec-conflict` issues from the M0 tasks, each with an interim reading in an ADR that the code already follows. The fifth, #81, was found by GATE-23's loop of record (ADR-46): the twin rule as written turns two passing providers inconclusive whenever a twin runs a shared control live.

`crates/acn-hyp` is in the frozen set, so every change to it is Class C. The maintainer's plan of 2026-11-10 settles all five in one spec amendment, taking the option each issue recommends, with the code to follow in one `env-change` PR after gate M1 is signed.

The adversarial review of the amendment's first draft found that three of those recommended options were not enough as written. This ADR records the text as it stands after that review, and marks where it goes beyond what an issue proposed.

## Decision
SPEC 080 becomes v0.3. No requirement ID is added or removed.

| Issue | Requirement | Change |
|---|---|---|
| #18 | HYP-9, HYP-20 | In a file with no `provider` parameter, `pins.models` has exactly one key, a real backend's `backend` value. HYP-20 checks the model pin on real-backend bundles only; a mock bundle's model is not looked up. |
| #13 | HYP-10, HYP-15 | A confidence level whose two bound indices cross has no interval, and a file that uses one fails to load. This is ADR-19's reading. |
| #15 | HYP-24 | A provider value with any bundle in the set has every one of its slices counted; a slice of it with no bundle is inconclusive. This is ADR-20's reading. |
| #10 | HYP-27 | Lint probes interval bounds with signed values. It also fails a guard that is not false for a complete run. |
| #81 | HYP-22, HYP-28 | The twin rule judges a cell only when it is twinned: both arms live, with at least half the replicates paired. A relative divergence whose numerator is exactly zero is zero. |

**Where the text goes beyond the issue.**
- **#18, one real backend.** The issue proposed keying by backend. Alone, that would let a file with no `provider` parameter pin several real backends. HYP-20 refuses a set that mixes backends, so an operator could make one complete, pinned verdict per backend and keep the one that passes, with no `not_run` and no `partial-providers` to show the rest. Such a file therefore names exactly one real backend. A hypothesis about several providers varies `provider`.
- **#18, the mock is not pinned.** SPEC 100 §7 question 3 was filed with this issue: a pinned file refused its own mock bundles, because a provider's model cannot be both the mock profile and the real model. A pinned file could then have no L1 loop and no twin, yet HYP-23 keeps an unpinned one from being cited. The model pin now applies to real-backend bundles only, as that question recommended. Scenario and workload pins still apply to every bundle.
- **#10, bounds only.** The issue proposed signed probes for every effect-valued term. Lint has no slot for `effect` or `rel_effect`: it computes them from their arms' probe values, so they are negative already. Giving them free signed slots would weaken lint: `effect(q) > 0 and rel_effect(q) < 0` can never fire, and would get a witness. Only `ci_low` and `ci_high` are probed, and only they need the signed set.
- **#81, one zero rule.** The lab note proposed "identical means are divergence 0". The effect's divergence has a different base, the sim control's value, so the rule is stated on the numerator: a relative divergence whose numerator is exactly zero is zero, for an arm and for the effect alike.

**Options not taken.**
- #18: requiring every `real-api` file to vary `provider`, or skipping the model pin when there is no provider. The second leaves the model unpinned.
- #81: judging decision cells only. A twinned cell that is not a decision cell and diverges is still evidence against the simulator, and fails the rule.

## Consequences
- **The code is behind the spec until its `env-change` PR**, which is held until gate M1 is signed so that the gate's evidence and the code that judged it are one build. `trace-scope.toml` goes on listing these IDs as implemented in the meantime, and `trace-check` does not see the gap; this ADR is where it is recorded.
  - **HYP-22.** The code fails a cell with one live arm, for a quantity read through an effect built-in, and treats any zero base as undefined. Three tests assert that behaviour and change with the code: two in `crates/acn-hyp/tests/verdict_semantics.rs` (the control's twin missing; no replicate index completed in both modes) and one in `crates/acn-hyp/tests/loop_gating.rs` (a live control without a live treatment records `twin_failed`).
  - **HYP-9, HYP-20.** The code refuses every key of `pins.models` in a file with no `provider` parameter, and looks a mock bundle's model up like any other.
  - **HYP-27.** The code searches first with non-negative probes and then with every term but `noise_floor` signed, and reports a witness found only in the second search as a warning naming issue #10. v0.3 asks for one search, with the bounds signed and nothing else.
  - **HYP-28.** The code reports `twinned` for decision cells.
  
  HYP-15 and HYP-24 already behave as v0.3 says (ADR-19, ADR-20), and lint already fails a guard that is not false for a complete run (ADR-18).
- **GATE-23's loop of record is not re-judged.** Its verdicts `bf17daba…` and `8401912d…` stay as committed, made by the v0.2 rule on build `60d0899c…`. ADR-46 still explains its `twin_failed` reasons. A twin made after the code change will not carry them.
- **What changes in a verdict.**
  - Under the twinned-only rule no divergence figure changes. What changes is which cells count as failing, and so the L2 status of a provider whose control was twinned. A slice can read `pass` where v0.2 gave `inconclusive`.
  - Under the zero rule a figure does change: a relative divergence of two exactly equal values over a zero base goes from undefined to 0.
- **The zero rule is narrow.** A quantity that is zero in sim and not in live is still undefined under a relative tolerance and still needs an `abs` tolerance, as `lab/hypotheses/p4-twin.toml` gives `cached_token_ratio`. The comparison is exact equality of two means, each summed in replicate order; SPEC 080 does not yet say that order for the twin's means in so many words, and the code PR should.
- **What the twin rule does not vouch for.** A cell that is not a decision cell can be left untwinned, by running one arm live or too few replicates, and then nothing is said about it. That was so under v0.2 as well, where leaving the cell out of the live set did the same. The rule vouches for the cells a verdict rests on: an untwinned decision cell makes the verdict `partially-twinned`, and not citable when `twin_required` is true (HYP-23).
- **A provider that cannot run one of its slices can never be reported** under HYP-24. That is intended: the alternative is the half-submitted provider.
- **A file about one real backend and the mock is the only single-provider shape.** POC specs that need a single provider without a `provider` parameter write their pins that way.
- **The four `spec-conflict` issues and #81 close with the code PR**, not with this one.
