# ADR-29 — Maintainer decisions after the POC 4 mock run (2026-10-05)

**Status:** accepted (maintainer decision, recorded by the agent; `spec-change`). **IDs affected:** SPEC 080 §6 question 1; SPEC 100 P4-9, §7 questions 1 and 2; HAR-60 (issue #27); CON-21, CON-22; TASKS T06c, T06d, T07, T08.

## Context
The POC 4 L1 run of record on the mock passed in both priced slices (`docs/runs/2026-10-05-p4-l1-mock.md`). The agent listed four questions for the maintainer: the falsifier statistic, the live-run settings, the first measured trace, and what to do next. The maintainer answered them on 2026-10-05.

## Decision
1. **The statistic and the grid stay as they are.**
   - `hypotheses/p4.toml` keeps its falsifier: the largest knob effect against the control's split-half 95% noise floor.
   - It keeps its full grid over every provider value, inert breakpoint placements included. There are no conditional domains and no `p4` successor.
   - SPEC 080 §6 question 1 is settled this way, and SPEC 100 P4-9 drops it as a precondition.
   - The calibration concern recorded there stays on record as a known limitation of the statistic, not as a blocker.
2. **Mocks first.** The project builds the substrate first and runs on the mock wherever a real component is not yet needed. Mock components are replaced by real ones later.
   - Live runs on real providers (SPEC 100 §5, T06c, T06d) are deferred, and with them the choice of models and pins and the spend estimate.
   - **The request settings for live runs (issue #27)** are the providers' typical defaults: the harness sends no sampling parameter a provider does not need. The issue records this; the change lands with the live tier.
3. **The first measured trace is a published one.** It is "5G-IANA: Nokia Testbed – Drivetest results for UL & DL Throughput and RTT with a OnePlus9 UE" (Zenodo record 12664724, CC BY 4.0).
   - **Contents:** a 1.5-hour drive test on a 5G testbed with 179 RTT summaries (average, min, max, standard deviation, loss) and throughput.
   - **How it differs from the plan:** it is a drive, not the walk that T08 planned, and it is not our own capture.
   - **What T08 does with it:** carries it into `scenarios/measured/` with the provenance file CON-21 requires, under its licence, and with location reduced to a class.
   - **Fallback:** where a later spec needs a trace this one cannot give, a synthetic trace is written under `scenarios/synthetic/` and labelled as guessed.
4. **The substrate continues on the mock.** The M0 gate (T07) records the mock deliverables and lists the live items as deferred. The gate closes only by the maintainer's merge (CON-22). After that, the M1 tasks proceed with mock components.

## Consequences
- SPEC 080 §6 question 1 and SPEC 100 §7 question 2 are closed. SPEC 100 P4-9 keeps its other preconditions for a citable live verdict: pins, approvals and one build per campaign.
- `TASKS.md` defers T06c and T06d, and names the 5G-IANA trace for T08.
