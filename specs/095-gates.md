# SPEC 095 — Milestone gates

**Status:** Draft v0.2 (October 2026; v0.2: §3, gate M1, for T17; v0.1: §1 and §2, gate M0, for T07). The sections for M2 to M4 are written by the task that closes each gate, with criteria numbered in each gate's band (GATE-1). **Inherits:** SPEC 000 (CON-16, CON-18, CON-22, CON-26). **Prefix:** GATE.
**Purpose:**
- make a gate's exit evidence a list that a machine can check against its record;
- say how a criterion that the maintainer chose not to meet yet is carried, rather than dropped;
- define gate M0 as amended by ADR-29: mock deliverables now, live provider runs deferred;
- define gate M1: the substrate on mocks, a sim result reproduced byte for byte by a second machine, a live twin within its declared tolerance, and the kit.

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

## 3. Gate M1 — the substrate on mocks

M1 delivers the emulator in `sim` and through the live proxy, the workload generator, the control plane, attribution, regeneration from a `run_id`, and the kit (PLAN.md, M1). It is still built on mocks (ADR-29). The maintainer decided on 2026-10-08 to defer GATE-14 again, to M2 (ADR-42): M1 closes on the substrate.

**GATE-20** **Workspace and gates.** The workspace builds, and `tools/ci.sh` (the CON-9 gates) and the required status checks of `main`'s branch protection pass on the head commit of the PR that adds the record.

**GATE-21** **Substrate crates.** `acn-emu` in `sim` and live (SPEC 020), `acn-gen` (SPEC 050), `acn-ctl` (SPEC 070), `acn-attrib` (SPEC 090) and regeneration from a `run_id` (SPEC 140) are implemented: their IDs are listed in `trace-scope.toml` and cited by tests (CON-12).

**GATE-22** **Sim is bit-identical.** A scenario run twice in `sim` gives byte-identical bundles (CON-5(c)), and a `sim` bundle regenerates byte for byte from its `run_id` on the same build (P16-6). The record cites the acceptance suites that show it: `tests/accept/trace_determinism.rs`, `tests/accept/emu_sim.rs`, `tests/accept/harness_sim.rs`, `tests/accept/gen_sim.rs` and `tests/accept/kit.rs`.

**GATE-23** **The live twin within tolerance.** A loop on the mock, whose report is committed as P16-20 says, is twinned in `live` (LOOP-12), with its twin and verdicts committed too. Its hypothesis declares `sim_live_tolerance` for every quantity its predicate reads, and may be a candidate under `lab/hypotheses/` (LOOP-10). On its evidence page, every twinned quantity is within its tolerance (CON-25). A quantity outside it is not met: the criterion is then deferred under an ADR of its own (GATE-3). The verdicts are `mock-gated` and are not results (CON-26).

**GATE-24** **A second machine regenerates the bundle.** A second machine of the target, and with the `build_hash`, that the kit's reference manifests record, built from a clean checkout of the kit's tag (P16-10), regenerates every reference manifest `identical`, recorded under `docs/runs/` as P16-11 says.

**GATE-25** **The kit.** The kit's release tag carries `kit/manifests/`, with the manifests of the `sim` bundles of GATE-23's loop and of CI's reference run, all made on one build from a clean checkout of the tag, and the README quickstart (P16-10, P16-30). CI's cross-target comparison at the tagged commit, run by `workflow_dispatch` or on `main`, is recorded under `docs/runs/` (P16-12).

**GATE-26** **The working-group package.** The kit, the POC 4 table on the mock with its `mock-gated` label (shown, not cited: CON-26), and one exploratory turn-transport number from a lab note under `docs/lab/` (CON-23) are presented to the working group. A short account of the presentation, where and when and what was shown, is committed under `docs/wg/`, and the record cites it.

**GATE-27** **Sign-off.** The maintainer merges the PR that adds `docs/gates/M1.md` (GATE-4).

GATE-14, carried from M0 (GATE-5), appears in the M1 record as deferred under ADR-42, due at M2.

## 9. Acceptance tests

- `tests/accept/gates.rs`:
  - **GATE-1 to GATE-6** on every record under `docs/gates/`, and on fixture records that break each rule. The fixtures include a missing, extra or duplicated criterion, an unknown status, a `met` row without a file, a path that does not exist, is absolute, climbs out with `..` or names a directory, and an unpaired backtick. They also include an ID with no run record, written with or without backticks, and a deferral without an ADR, with an ADR that does not mention the criterion, with no `due at`, or to a gate that is not later or beyond M4. Finally, a missing or unresolvable base, a deferred criterion not carried forward, a second deferral under the same ADR, a record named with a leading zero, a record for a gate with no section, and a section with no criteria. Each fixture must produce exactly the expected problem.
  - **The substance of M0's criteria** where a test can see it:
    - GATE-11: `trace-scope.toml` lists SPEC 010, 030, 040, 080 and 085.
    - GATE-12: the POC 4 suite cites CON-18.
    - GATE-13: the cited run record holds the cited IDs and says `mock-gated`.
    - GATE-15: every cited trace under `scenarios/measured/` loads under EMU-64.
  - **The substance of M1's criteria** where a test can see it:
    - GATE-21: `trace-scope.toml` lists the implemented IDs of SPEC 020, 050, 070, 090 and 140, and `cargo xtask trace-check` finds each cited.
    - GATE-22: the cited suites exist and cite CON-5 and P16-6.
    - GATE-23: the cited loop's report, twin and verdicts are committed, its evidence page is current, and no twinned quantity is outside its tolerance.
    - GATE-24 and GATE-25 (P16-10, P16-11): the cited run records name the tag, the target, the `build_hash` and `identical`, and every reference manifest is in canonical form, of `sim`, and of that `build_hash`.
    - GATE-26: the cited account exists under `docs/wg/`.
  - GATE-10 and GATE-20 are the CI run itself. GATE-16, GATE-27, the substance of GATE-26 and the process rules of GATE-4 are the maintainer's.

## 10. Open questions

1. GATE-14 was due at M1 (ADR-31) and is deferred again, to M2 (ADR-42). Live runs wait for the maintainer to approve models and spend.
2. Whether a gate record should also carry a machine-readable form (TOML) beside its table. The table is enough while the records are few.
