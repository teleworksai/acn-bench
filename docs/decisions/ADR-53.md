# ADR-53 — GATE-26, the working-group package, is deferred to M2

**Status:** proposed with the M1 record, and accepted by the maintainer's merge of it (GATE-27). **IDs affected:** GATE-26, GATE-27; by reference GATE-3, GATE-5, CON-23, CON-26.

## Context
GATE-26 asks that the kit, the POC 4 table on the mock and one exploratory turn-transport number be presented to the working group, and that a short account of the presentation, where and when and what was shown, be committed under `docs/wg/`.

When the rest of gate M1 was ready, no working-group meeting had been arranged. The maintainer's plan of 2026-11-10 left the meeting open and said what to do in that case: defer the criterion under an ADR of its own, as GATE-3 allows, and close M1 on the substrate.

Everything GATE-26 would present exists:
- the kit, at its tag, with its reference manifests (ADR-52);
- the POC 4 table on the mock, with its `mock-gated` label, on `docs/evidence/p4.md`;
- the turn-transport numbers of two lab notes, `docs/lab/2026-09-21-turn-transport.md` and `docs/lab/2026-11-10-turn-transport-2.md`.

What is missing is the presentation itself, which is not the repository's to produce.

## Decision
- **GATE-26 is deferred, due at M2.** The M1 record carries it as `deferred` under this ADR. The M2 record carries it again, as `met` with the account under `docs/wg/`, or deferred under another ADR (GATE-5).
- **The account is written by whoever presents**, not drafted ahead of the meeting: it records what happened.
- **Nothing is presented as a result before then.** The POC 4 table is `mock-gated` and is shown, never cited (CON-26); the lab numbers are exploratory (CON-23).

## Consequences
- **M1 closes on the substrate and the kit**, with two criteria carried to M2: GATE-14 (live providers, ADR-42) and GATE-26.
- **M2 gains nothing else from this.** Its own criteria are not written yet (SPEC 095).
- **If a meeting is arranged before the M1 record merges**, this ADR is withdrawn and GATE-26 is met in the M1 record in the ordinary way.
