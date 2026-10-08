# ADR-42 — M1 closes on the substrate; GATE-14 deferred to M2

**Status:** accepted (T17.1, spec-change). **IDs affected:** GATE-14, GATE-20 to GATE-27.

## Context
GATE-14, POC 4 on at least two real providers with a verdict for each, was deferred at M0 under ADR-29 and ADR-31, due at M1. M1's own deliverables (PLAN.md) are the substrate: the emulator in `sim` and live, the generator, the control plane, attribution, regeneration and the kit. All of them are built and run on the mock. Live provider runs need API keys, approved models and spend, and none has been arranged. GATE-5 lets a criterion be deferred again only under an ADR of its own.

## Decision
The maintainer decided on 2026-10-08 that:
- **GATE-14 is deferred again, due at M2.** The M1 record carries it as deferred under this ADR (GATE-3, GATE-5). The live POC 4 protocol (SPEC 100 P4-9 to P4-11) runs once the maintainer approves models and spend.
- **SPEC 095 gains §3, gate M1,** before the record. Its criteria are taken from PLAN.md's M1 exit row:
  - workspace and gates (GATE-20);
  - the substrate crates (GATE-21);
  - `sim` bit-identity (GATE-22);
  - the live twin within tolerance (GATE-23);
  - second-machine regeneration (GATE-24);
  - the kit (GATE-25);
  - the working-group package (GATE-26);
  - sign-off (GATE-27).
- **T13b waits.** The plain-RPC control (T13b), which needs a Class C SPEC 010 addition, is scheduled after M1.

Interpretations made in §3, after a read-only review:
- **GATE-23 needs a hypothesis that twins what it decides.** POC 4's frozen file twins `cached_token_ratio` and `ttft_p50_ms`, but its predicate reads `cost_per_success`, which has no tolerance. Its twin would therefore never test what its verdict rests on, and changing the frozen file is Class C. T17.2 adds a candidate, `lab/hypotheses/p4-twin.toml`: POC 4's design with `twin_required = true` and a tolerance on every quantity its predicate reads. GATE-23 allows a candidate (LOOP-10).
- **GATE-23 is met only within tolerance.** PLAN.md asks that the live twin agree within the declared tolerance. A twin outside it is an honest negative, deferred under an ADR of its own, never written up as met.
- **The working group sees the mock table.** PLAN.md's package names a per-provider POC 4 table. With GATE-14 deferred, the package shows the mock table with its `mock-gated` label, never cited (CON-26). The per-provider table comes with GATE-14 at M2.
- **The kit's reference manifests are `sim` only** (SPEC 140 §1): a loop's live bundles are not among them. They are made on one build from a clean checkout of the tag, and GATE-24's second machine must match that target and `build_hash`.
- **CI does not run on tags,** so the cross-target comparison of the kit is a `workflow_dispatch` run, or a run on `main`, at the tagged commit.
- **GATE-26's evidence** is a short committed account under `docs/wg/`, so that its row can cite a repository file (GATE-2).

## Consequences
- **What the M1 record needs:**
  - a loop of record on a hypothesis that declares `sim_live_tolerance`, twinned and committed with its evidence page (GATE-23);
  - a release tag with `kit/manifests/` (GATE-25);
  - a second-machine run record (GATE-24);
  - the maintainer's account of the working-group presentation (GATE-26).
- **The order is the gate's.** No M2 task starts before the maintainer merges the M1 record (GATE-4).
- **POC 4's only result stays mock-gated:** a model-of-caching table that is not cited (CON-26).
