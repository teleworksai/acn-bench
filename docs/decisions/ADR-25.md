# ADR-25 — T06b: the POC 4 workloads, the mock suite and the run of record

**Status:** accepted (T06b; Class A with a Class C part in `crates/acn-hyp`). **IDs affected:** P4-1 to P4-5, P4-7, P4-8; CON-18; LOOP-11; HAR-17, HAR-30.

## Context
SPEC 100 v0.1 asks for three workloads, a mock suite and an L1 run of record. Implementing them needs these readings.

## Decision
- **The workloads** are `workloads/p4-{coding,retrieval,fanout}.toml`, three tasks of three or four turns each.
  - **System prompts.** Each system prompt is a numbered list of engineering rules of about 8 700 bytes, about 2 180 estimated tokens. This meets P4-2 with a margin of about 6%.
  - **Tools.** Each tool comes first in exactly one task, as MLM-40 requires.
  - **`run_tests`** has class `testbed`: HAR-60 has no `exec` class.
  - **Thresholds.** `coding` uses `compact_at_tokens = 2600` and `read_cost_threshold_tokens = 1500`, so that every task compacts under both triggers on `mock-explicit` and `mock-auto`.
- **The stable prefix is measured from the file**, as ⌈`system_prompt` bytes / 4⌉. The file's prompt carries neither the isolation marker nor the timestamp line, since the harness adds both (HAR-11, HAR-42). So this is the byte count of the system message that P4-2's definition subtracts down to.
- **"Changes the requests" is checked through the mock's accounting.**
  - The method: P4-8(b) runs one replicate per knob value with the same seed, on the same mock, and compares each call's input, cache-read, cache-write and output tokens with the control run's.
  - Why it is sound in one direction: identical requests give identical accounting on a deterministic mock, so a difference proves that the requests differed.
  - Why not compare request bytes: the acceptance crate cannot reach the harness's request bytes, and capturing them would add a recording server to the run path. The harness's own suite checks request bytes for HAR-17.
- **"Every task compacts"** is read as: every session has a turn whose `compaction` is not `none`, since a session is one task of one replicate (HAR-30).
- **The fixture** is `tests/accept/fixtures/p4t-timestamp.toml`. SPEC 100 named it `p4-timestamp.toml` with id `p4t`, which HYP-1's `<id>-<slug>` rule refuses, so this PR also corrects the name in the spec.
- **The report's effects carry replicate counts.** Each `control_effect` entry of `report.json` gains `treatment_replicates` and `control_replicates`, as CON-18 and P4-8(d) require. The layout keeps `acn-bench/loop-report/v1`, because no report has been cited yet.
- **P4-7's acceptance** is checked without running the grid. `hypotheses/p4.toml`, frozen, is refused at budget 1 547 with `budget_too_small`. At 1 548 it reaches the executor, which this test refuses, so every check before the first batch passed and nothing was written.

## Consequences
- P4-1 to P4-5, P4-7 and P4-8 are in scope.
- The L1 run of record is made with the release build of this PR, from the workspace root. Its loop_id, final verdict and `acn evidence verify` result are attached to the PR.
- P4-9 to P4-11 wait for T06c, T06d and the settled falsifier.
