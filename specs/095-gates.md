# SPEC 095 — Milestone gates

**Status:** Draft v0.1 (October 2026). Partial: §1, the rules for every gate, and §2, gate M0, are written, for T07. The sections for M1 to M4 are written by the task that closes each gate (T17 for M1) and number their criteria from 20 (M1), 30 (M2), 40 (M3) and 50 (M4). **Inherits:** SPEC 000 (CON-16, CON-18, CON-22, CON-26). **Prefix:** GATE.
**Purpose:**
- make a gate's exit evidence a list that a machine can check against its record;
- say how a criterion that the maintainer chose not to meet yet is carried, rather than dropped;
- define gate M0 as amended by ADR-29: mock deliverables now, live provider runs deferred.

## 1. Every gate

**GATE-1** Each gate's section MUST list its exit criteria as numbered requirements. A gate's record `docs/gates/M<n>.md` MUST hold one table whose rows are exactly that gate's criteria, one row each, with three columns: the criterion ID, its status, and its evidence. The status is `met` or `deferred`.

**GATE-2** A `met` row's evidence MUST name at least one repository path, in backticks, and every backticked repository path in the record MUST exist at the record's commit. A run, loop or verdict ID cited in the record (64 lowercase hex characters) MUST appear in a run record under `docs/runs/`, so that the bundles behind it can be regenerated (CON-22, CON-31).

**GATE-3** A `deferred` row MUST name the ADR that records the maintainer's decision to defer it (`ADR-<n>`, an existing file under `docs/decisions/`) and the gate at which it is next due (`M<k>`, with k greater than the record's gate). A criterion MUST NOT be deferred without such an ADR.

**GATE-4** A record MUST state the commit at which its evidence was checked. The gate closes only when the maintainer merges the PR that adds the record (CON-22, CON-16(d)). An agent MUST NOT merge that PR, and MUST NOT start a task listed after the gate in `TASKS.md` before the merge. Lab work is not gated (CON-23).

**GATE-5** A criterion deferred at gate M<n> MUST appear again in the record of the gate it is due at, as `met` or again `deferred` under GATE-3. A row carried forward this way keeps its original ID.

**GATE-6** The acceptance suite `tests/accept/gates.rs` MUST check GATE-1, GATE-2, GATE-3, the commit of GATE-4 and GATE-5 for every record under `docs/gates/`, against this spec. It MUST also refuse a record for a gate whose section is not written.

## 2. Gate M0 — bootstrap and first result

M0 delivers the workspace and its gates, the trace schema, the mock inference server, the harness with its knobs, verdicts and the loop runner, the POC 4 acceptance suite and run, and a first measured trace. The maintainer decided on 2026-10-05 that the substrate is built on mocks first and that live provider runs come later (ADR-29). So M0 closes on the mock run, and the live criterion is carried as deferred.

**GATE-10** **Workspace and gates.** The workspace builds, and `tools/ci.sh` (the CON-9 gates) and the required CI checks of ADR-6 pass on the record's commit.

**GATE-11** **Substrate crates.** `acn-trace` (SPEC 010), `acn-mockllm` (SPEC 030), `acn-harness` with the cache-discipline knobs (SPEC 040), `acn-hyp` with verdicts on a bundle (SPEC 080) and the L1 loop runner with its evidence chain (SPEC 085) are implemented: their IDs are listed in `trace-scope.toml` and cited by tests (CON-12).

**GATE-12** **POC 4 acceptance suite.** The POC 4 suite (SPEC 100) runs its treatments against the control its hypothesis names, under the same scenario, and reports the treatment-minus-control effect with its replicate count (CON-18).

**GATE-13** **POC 4 on the mock.** A POC 4 run of record on the mock backend is recorded under `docs/runs/`, with its loop and verdict IDs. Its verdicts are labelled `mock-gated` and are not results (CON-26). `acn evidence verify` succeeded on it.

**GATE-14** **POC 4 on real providers.** POC 4 runs on at least two real providers that report cached-token counters, with a verdict for each provider (SPEC 100 P4-9 to P4-11).

**GATE-15** **A measured trace.** At least one measured trace with its provenance is in `scenarios/measured/`, and loads under SPEC 020 EMU-64 (CON-21).

**GATE-16** **Sign-off.** The maintainer merges the PR that adds `docs/gates/M0.md` (GATE-4).

## 9. Acceptance tests

- `tests/accept/gates.rs` — GATE-1 to GATE-6 on every record under `docs/gates/`, and on fixture records that break each rule: a missing or extra criterion, an unknown status, a `met` row without a path, a path that does not exist, an ID with no run record, a deferral without an ADR or with a gate that is not later, a missing commit, a deferred criterion not carried forward, a record for a gate with no section.

## 10. Open questions

1. When GATE-14 is due at M1, whether the M1 record meets it or defers it again depends on when live runs are scheduled (ADR-29). That is the maintainer's decision, recorded in an ADR either way.
2. Whether a gate record should also carry a machine-readable form (TOML) beside its table. The table is enough while the records are few.
