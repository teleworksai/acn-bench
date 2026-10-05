# ADR-25 — T06b: the POC 4 workloads, the mock suite and the run of record

**Status:** accepted (T06b; Class A with a Class C part in `crates/acn-hyp`). **IDs affected:** P4-1 to P4-5, P4-7, P4-8; CON-18; LOOP-11; HAR-17, HAR-30.

## Context
SPEC 100 v0.1 asks for three workloads, a mock suite and an L1 run of record. Implementing them needs these readings.

## Decision
- **The workloads** are `workloads/p4-{coding,retrieval,fanout}.toml`, three tasks each: five turns per task in `coding`, three or four elsewhere.
  - **System prompts.** Each system prompt is a numbered list of engineering rules of about 8 700 bytes, about 2 180 estimated tokens. This meets P4-2 with a margin of about 6%.
  - **Tools.** Each tool comes first in exactly one task, as MLM-40 requires.
  - **`run_tests`** has class `testbed`: HAR-60 has no `exec` class.
  - **Thresholds.** `coding`'s tasks have five turns each, with `updates` at turn 1.
    - `compact_at_tokens = 3800` makes every task compact under `window_full`, at turn 2 or later, so the result an update rewrites is still in context. At 2 600 a task compacted on nearly every turn, and the backfill knob had nothing to rewrite.
    - `read_cost_threshold_tokens = 1500` makes every task compact under `read_cost_threshold` with the timestamp on.
    - `retrieval` has `compact_at_tokens = 20000`: it never compacts, so its updates (in the last turn) always find their result.
  - **`retrieval`'s tool descriptions** are detailed enough that its tool block, as JSON, is about 1 130 estimated tokens. That is above the mock's 1 024-token minimum, so a `system_and_tools` breakpoint caches something. Below it, that placement was bit-identical to `system_only` on every workload.
- **The stable prefix is measured from the file**, as ⌈`system_prompt` bytes / 4⌉. The file's prompt carries neither the isolation marker nor the timestamp line, since the harness adds both (HAR-11, HAR-42). MLM-10's prompt bytes wrap and escape the system message, so this measure is a lower bound on P4-2's quantity. If the lower bound meets the threshold, so does the quantity.
- **"Changes the requests" is checked through the mock's accounting.**
  - The method: P4-8(b) runs one replicate per knob value with the same seed, on the same mock, and compares each call's input, cache-read, cache-write and output tokens with the control run's.
  - Why it is sound in one direction: identical requests give identical accounting on a deterministic mock, so a difference proves that the requests differed.
  - What it covers: every non-default value of every knob of `hypotheses/p4.toml`'s control, taken from the file, so a seventh knob or a fourth placement would be checked.
  - Why not compare request bytes: the acceptance crate cannot reach the harness's request bytes, and capturing them would add a recording server to the run path. The harness's own suite checks request bytes for HAR-17.
- **"Every task compacts"** is read as: every session has a turn whose `compaction` is not `none`, since a session is one task of one replicate (HAR-30).
- **The fixture** is `tests/accept/fixtures/p4t-timestamp.toml`. SPEC 100 named it `p4-timestamp.toml` with id `p4t`, which HYP-2's `<id>-<slug>` rule refuses, so this PR also corrects the name in the spec. The test asserts that its measures, replicates, provider minimum, predicate and guard equal `hypotheses/p4.toml`'s.
- **The report's effects carry replicate counts.** Each `control_effect` entry of `report.json` gains `treatment_replicates` and `control_replicates`, as CON-18 and P4-8(d) require. The layout keeps `acn-bench/loop-report/v1`, because no report has been cited yet.
- **P4-4 stays out of scope while `hypotheses/p4.toml` is unpinned.** Its check is in `existing_files.rs`, but it is not yet in force.
- **P4-7's acceptance** is checked without running the grid. It uses the workspace's own `runs/`, because the frozen file must lie in the directory `runs/` lies in (ADR-23), and it only reads it. `hypotheses/p4.toml`, frozen, is refused at budget 1 547 with `budget_too_small`. At 1 548 it reaches the executor, which this test refuses, so every check before the first batch passed and nothing was written.
- **What the review found in the harness and the mock.**
  - Compaction summaries are empty on the mock (issue #24).
  - Forked children call the `subagent` tool on the mock (issue #25).
  - Shuffled tool order changes which tool the mock calls (issue #26).

  These make the run of record an artifact, so SPEC 100 v0.2 adds P4-12, which holds it until T06b2 settles them. The live request surface (`temperature`, `max_tokens`, length stops, issue #27) is T06d's.

## Consequences
- P4-1, P4-2, P4-3, P4-5, P4-7 and P4-8 are in scope.
- The L1 run of record is made after T06b2, with that PR's release build, from the workspace root.
- P4-9 to P4-11 wait for T06c, T06d and the settled falsifier.
