# ADR-31 — T07: SPEC 095's gate rules and the M0 record

**Status:** accepted (T07; `spec-change`, SPEC 095 Draft v0.1). **IDs affected:** GATE-1 to GATE-6, GATE-10 to GATE-16; CON-7, CON-16, CON-22, CON-23. **Follows:** ADR-29, ADR-30.

## Context
CON-22 says a gate closes by a human-merged PR that adds `docs/gates/M<n>.md`, "recording the evidence the gate spec requires". SPEC 095 was unwritten, so nothing said:
- what evidence a gate requires;
- what a record must look like;
- what happens to a criterion the maintainer chooses not to meet yet.

PLAN.md §5 put live runs on at least two real providers in M0's exit evidence. ADR-29 deferred those runs and kept the rest of the substrate on mocks.

## Decision
- **SPEC 095 is written partially.** It has §1, rules for every gate (GATE-1 to GATE-6), and §2, gate M0 (GATE-10 to GATE-16). Each later gate's section is written by the task that closes it. Their criteria are numbered from 20 for M1, 30 for M2, and so on.
- **M0's criteria follow PLAN.md §5**, split one per line so that each can be met or deferred on its own. The criteria are:
  - the workspace and its gates;
  - the substrate crates;
  - the POC 4 suite with its control;
  - the mock run of record;
  - the real-provider runs;
  - a measured trace;
  - sign-off.
- **A record is a table, checked by a test.** Each row holds an ID, a status (`met` or `deferred`) and evidence. `tests/accept/gates.rs` checks:
  - the rows against the spec's criteria;
  - that every backticked path exists;
  - that every 64-hex ID appears in a run record under `docs/runs/`;
  - that every deferral names an existing ADR and a later gate;
  - that every deferred criterion is carried into the record of the gate it is due at.
  - **The heuristics.** A backticked span counts as a path when it holds a `/` and no whitespace. A deferral's gate is its first `M<k>` with k above the record's own gate.
- **Deferral is explicit and carried forward (GATE-3, GATE-5).** A criterion cannot be dropped. It can only be deferred, by an ADR recording the maintainer's decision, and it reappears at the gate it is due at. GATE-14, the real-provider runs, is deferred by ADR-29 to M1. That matches T17, M1's gate, whose WG package already names a per-provider POC 4 table. The M1 record meets GATE-14 or defers it again under a new ADR.
- **GATE-15 is met by the published trace.** PLAN.md asked for our own phone-tethered walk. ADR-29 chose the published 5G-IANA drive test, and ADR-30 imported it. The record says so, and the deferred items list our own capture under T41.
- **GATE-16, sign-off, is the merge itself.** The row cites the record. It is true once the maintainer merges, and the agent does not merge (GATE-4, CON-16(d)).
- **What closing M0 turns on** is listed in the record:
  - CON-7's closed frozen set, with an adversarial review for every `env-change`;
  - `pr-check` failing, rather than advising, on a frozen-set change without the label (ADR-6), once `docs/gates/M0.md` is on the base branch;
  - the start of M1.

## Consequences
- The M0 PR is opened, reviewed and fixed by the agent loop and left for the maintainer's merge. No M1 task starts before it. Lab work is not gated.
- A wrong path or an unrecorded run ID in any gate record now fails CI.
- Each later gate's task must write its SPEC 095 section before its record, or the suite refuses the record (GATE-6).
