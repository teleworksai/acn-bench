# ADR-50 — SPEC 080 v0.3: five open conflicts settled in one amendment

**Status:** accepted (`spec-change`; issues #10, #13, #15, #18, #81). **IDs affected:** HYP-9, HYP-15, HYP-20, HYP-22, HYP-24, HYP-27; by reference HYP-13, HYP-21, HYP-23, CON-25, LOOP-12, GATE-23.

## Context
SPEC 080 had five open issues against it. Four are `spec-conflict` issues from the M0 tasks, each with an interim reading in an ADR that the code already follows. The fifth, #81, was found by GATE-23's loop of record (ADR-46): the twin rule as written turns two passing providers inconclusive whenever a twin runs a shared control live.

`crates/acn-hyp` is in the frozen set, so every change to it is Class C. The maintainer's plan of 2026-11-10 settles all five in one spec amendment, taking the option each issue recommends, with the code to follow in one `env-change` PR after gate M1 is signed.

## Decision
SPEC 080 becomes v0.3. No requirement ID is added or removed.

| Issue | Requirement | Change |
|---|---|---|
| #18 | HYP-9, HYP-20 | `pins.models` is keyed by provider value in a file with a `provider` parameter, and by backend in a file with none. HYP-20 looks a bundle's model up under the same key. A single-provider file can then be pinned and judged. |
| #13 | HYP-15 | A confidence level whose two bound indices cross has no interval, and a file that uses one fails to load. At 10 000 resamples that is every level at or below 0.0001. This is ADR-19's reading. |
| #15 | HYP-24 | A provider value with any bundle in the set has every one of its slices counted; a slice of it with no bundle is inconclusive. This is ADR-20's reading. |
| #10 | HYP-27 | Lint probes effect-valued terms (`effect`, `rel_effect`, `ci_low`, `ci_high`) with signed values, and quantity terms and `noise_floor` with non-negative ones, so a falsifier about a negative effect has a witness. Lint also fails a guard that is true for a complete run. |
| #81 | HYP-22 | The twin rule judges a cell only when it is twinned: both arms live, with at least half the replicates paired. "Twinned" is now defined for every cell, not for decision cells only. A relative divergence over a zero base is zero when the live mean is zero too, and undefined otherwise. |

**Options not taken.**
- #10: probing every term with signed values. It is simpler, but a quantity such as a latency or a token count is never negative, and the larger probe set costs lint its exhaustive search sooner.
- #18: requiring every `real-api` file to vary `provider`, or skipping the model pin when there is no provider. The second leaves the model unpinned.
- #81: judging decision cells only. A twinned cell that is not a decision cell and diverges is still evidence against the simulator, and stays a failure.

## Consequences
- **The code is behind the spec on three points until its `env-change` PR**, which is held until gate M1 is signed so that the gate's evidence and the code that judged it are one build:
  - HYP-22: the code still fails a cell with one live arm, and treats a zero base as undefined;
  - HYP-9 and HYP-20: the code still refuses a backend as a key of `pins.models`;
  - HYP-27: the code finds a signed witness in a second search and reports it as a warning naming issue #10. It becomes a clean result.
  
  HYP-15 and HYP-24 already behave as v0.3 says (ADR-19, ADR-20), and lint already checks the guard (ADR-18).
- **GATE-23's loop of record is not re-judged.** Its verdicts `bf17daba…` and `8401912d…` stay as committed, made by the v0.2 rule on build `60d0899c…`. ADR-46 still explains its `twin_failed` reasons. A twin made after the code change will not carry them.
- **No divergence figure changes** under the HYP-22 change, on any loop. What changes is which cells are counted as failing, and so the L2 status of a provider whose control was twinned.
- **The zero-base rule is narrow.** It applies when both means are exactly zero. A quantity that is zero in sim and not in live is still undefined under a relative tolerance, and still needs an `abs` tolerance, as `lab/hypotheses/p4-twin.toml` gives `cached_token_ratio`.
- **An attempt to hide a diverging cell by submitting few live replicates of it** leaves the cell untwinned. If it is a decision cell, the verdict is `partially-twinned` and, with `twin_required`, not citable (HYP-23). If it is not, the verdict does not rest on it.
- **The four `spec-conflict` issues and #81 close with the code PR**, not with this one, since three of them still need code.
