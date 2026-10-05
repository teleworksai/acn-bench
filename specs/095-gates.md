# SPEC 095 — Milestone gates

**Status:** Draft v0.1 (October 2026). Partial: §1, the rules for every gate, and §2, gate M0, are written, for T07. The sections for M1 to M4 are written by the task that closes each gate (T17 for M1), with criteria numbered in each gate's band (GATE-1). **Inherits:** SPEC 000 (CON-16, CON-18, CON-22, CON-26). **Prefix:** GATE.
**Purpose:**
- make a gate's exit evidence a list that a machine can check against its record;
- say how a criterion that the maintainer chose not to meet yet is carried, rather than dropped;
- define gate M0 as amended by ADR-29: mock deliverables now, live provider runs deferred.

## 1. Every gate

**GATE-1** Each gate's section MUST list at least one exit criterion, each a `**GATE-<n>**` paragraph with n in the gate's band (10 to 19 for M0, 20 to 29 for M1, and so on). A gate's record `docs/gates/M<g>.md`, named with no leading zeros, MUST hold one table under its `## Exit criteria` heading, with three columns: the criterion ID, its status (`met` or `deferred`) and its evidence. Its rows MUST be exactly that gate's criteria and the criteria carried to it (GATE-5), one row each. Files under `docs/gates/` whose names start with `.` are not records.

**GATE-2** A `met` row's evidence MUST name at least one repository file in backticks. A backticked span is a repository path when it holds a `/` and no whitespace; it MUST be relative, MUST NOT contain a `..` component, and MUST name a file that exists in the tree that adds the record. Every run, loop or verdict ID in the record (a word of 64 lowercase hex characters) MUST appear as a word in one run record under `docs/runs/`, so that the bundles behind it can be regenerated (CON-22, CON-31). Backticks pair within a line.

**GATE-3** A `deferred` row MUST name, as `ADR-<n>`, an existing ADR under `docs/decisions/` that mentions the criterion's ID and records the maintainer's decision to defer it, and MUST say `due at M<k>`, where k is greater than the record's gate and at most 4. A criterion MUST NOT be deferred without such an ADR.

**GATE-4** A record MUST state its base: the commit on `main` that the PR adding it starts from, as `**Commit:**` and a commit hash. The evidence is checked on the tree that adds the record: its paths by the acceptance suite, and the required status checks of `main`'s branch protection on the PR's head commit before the merge. The gate closes only when the maintainer merges that PR (CON-22). An agent MUST NOT merge it except on the maintainer's instruction naming that PR (CON-16(d)), and MUST NOT start a task listed after the gate in `TASKS.md` before the merge. These two are process rules, verified by the maintainer, not by a test. Lab work is not gated (CON-23).

**GATE-5** A criterion deferred at gate M<g> MUST appear again in the record of the gate it is due at, under its original ID, as `met` or again `deferred`. A second deferral MUST name a different ADR from the first: each deferral is its own decision.

**GATE-6** The acceptance suite `tests/accept/gates.rs` MUST check GATE-1, GATE-2, GATE-3, the base of GATE-4 and GATE-5 for every record under `docs/gates/`, against this spec, and MUST refuse a record for a gate whose section is not written. Where the base can be resolved in the repository, it MUST be an ancestor of the commit under test.

## 2. Gate M0 — bootstrap and first result

M0 delivers the workspace and its gates, the trace schema, the mock inference server, the harness with its knobs, verdicts and the loop runner, the POC 4 acceptance suite and run, and a first measured trace. The maintainer decided on 2026-10-05 that the substrate is built on mocks first and that live provider runs come later (ADR-29). So M0 closes on the mock run, and the live criterion is carried as deferred.

**GATE-10** **Workspace and gates.** The workspace builds, and `tools/ci.sh` (the CON-9 gates) and the required status checks of `main`'s branch protection pass on the head commit of the PR that adds the record.

**GATE-11** **Substrate crates.** `acn-trace` (SPEC 010), `acn-mockllm` (SPEC 030), `acn-harness` with the cache-discipline knobs (SPEC 040), `acn-hyp` with verdicts on a bundle (SPEC 080) and the L1 loop runner with its evidence chain (SPEC 085) are implemented: their IDs are listed in `trace-scope.toml` and cited by tests (CON-12).

**GATE-12** **POC 4 acceptance suite.** The POC 4 suite (SPEC 100) runs its treatments against the control its hypothesis names, under the same scenario, and reports the treatment-minus-control effect with its replicate count and confidence interval (CON-18).

**GATE-13** **POC 4 on the mock.** A POC 4 run of record on the mock backend is recorded under `docs/runs/`, with its loop and verdict IDs. Its verdicts are labelled `mock-gated` and are not results (CON-26). `acn evidence verify` succeeded on it.

**GATE-14** **POC 4 on real providers.** POC 4 runs on at least two real providers that report cached-token counters, with a verdict for each provider (SPEC 100 P4-9 to P4-11).

**GATE-15** **A measured trace.** At least one measured trace with its provenance is in `scenarios/measured/`, and loads under SPEC 020 EMU-64 (CON-21).

**GATE-16** **Sign-off.** The maintainer merges the PR that adds `docs/gates/M0.md` (GATE-4).

## 9. Acceptance tests

- `tests/accept/gates.rs`:
  - **GATE-1 to GATE-6** on every record under `docs/gates/`, and on fixture records that break each rule. The fixtures include a missing, extra or duplicated criterion, an unknown status, a `met` row without a file, a path that does not exist, is absolute, climbs out with `..` or names a directory, and an unpaired backtick. They also include an ID with no run record, written with or without backticks, and a deferral without an ADR, with an ADR that does not mention the criterion, with no `due at`, or to a gate that is not later or beyond M4. Finally, a missing or unresolvable base, a deferred criterion not carried forward, a second deferral under the same ADR, a record named with a leading zero, a record for a gate with no section, and a section with no criteria. Each fixture must produce exactly the expected problem.
  - **The substance of M0's criteria** where a test can see it:
    - GATE-11: `trace-scope.toml` lists SPEC 010, 030, 040, 080 and 085.
    - GATE-12: the POC 4 suite cites CON-18.
    - GATE-13: the cited run record holds the cited IDs and says `mock-gated`.
    - GATE-15: every cited trace under `scenarios/measured/` loads under EMU-64.
  - GATE-10 is the CI run itself. GATE-16 and the process rules of GATE-4 are the maintainer's.

## 10. Open questions

1. GATE-14 is due at M1 (ADR-31). Whether the M1 record meets it or defers it again depends on when live runs are scheduled. That is the maintainer's decision; a second deferral needs its own ADR (GATE-5).
2. Whether a gate record should also carry a machine-readable form (TOML) beside its table. The table is enough while the records are few.
