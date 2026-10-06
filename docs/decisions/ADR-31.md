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
- **A record is a table, checked by a test.** The table sits under `## Exit criteria`, and backticks pair within a line. Each row holds an ID, a status (`met` or `deferred`) and evidence. `tests/accept/gates.rs` checks:
  - the rows against the spec's criteria;
  - that every backticked path exists;
  - that every 64-hex ID appears in a run record under `docs/runs/`;
  - that every deferral names an existing ADR and a later gate;
  - that every deferred criterion is carried into the record of the gate it is due at.
  - **The heuristics.**
    - A backticked span with a `/` and no whitespace is a path. It must be relative, must not use `..`, and must name a file, not a directory.
    - A 64-hex word anywhere in the record must be a word of one run record.
    - A deferral says `due at M<k>`, with k at most 4, and names an ADR that mentions the criterion.
    - Dotfiles under `docs/gates/` (the `.gitkeep`) are not records. Keeping `.gitkeep` means this PR changes no existing file there, so it needs no `env-change` label.
- **Deferral is explicit and carried forward (GATE-3, GATE-5).** A criterion cannot be dropped. It can only be deferred, by an ADR recording the maintainer's decision, and it reappears at the gate it is due at. A second deferral needs an ADR of its own. GATE-14, the real-provider runs, is deferred by ADR-29, which names no gate. This ADR sets it due at M1; the maintainer's merge of the M0 record confirms that. That matches T17, M1's gate, whose WG package already names a per-provider POC 4 table. The M1 record meets GATE-14 or defers it again under an ADR of its own.
- **GATE-15 is met by the published trace.** PLAN.md asked for our own phone-tethered walk. ADR-29 chose the published 5G-IANA drive test, and ADR-30 imported it. The record says so, and the deferred items list our own capture under T41.
- **GATE-16, sign-off, is the merge itself.** The row cites the record. It is true once the maintainer merges, and the agent does not merge (GATE-4, CON-16(d)).
- **What closing M0 turns on** is listed in the record:
  - CON-7's closed frozen set, with an adversarial review for every `env-change`;
  - `pr-check` failing, rather than advising, on a frozen-set change without the label (ADR-6), once `docs/gates/M0.md` is on the base branch;
  - the start of M1.

- **The base, and where evidence is checked (GATE-4).** A record cannot name a commit that already contains it. So `**Commit:**` names the base, the `main` commit the PR starts from. The suite checks that the base is an ancestor of the commit under test whenever it can resolve it; CI's shallow clone cannot, and then the check is skipped. Paths are checked on the tree that adds the record. The required checks are those of `main`'s branch protection, green on the PR head before the merge.
- **What a test can and cannot check.** `m0_criteria_hold_where_a_test_can_see_them` checks the substance of GATE-11, 12, 13 and 15:
  - the specs in scope;
  - the POC 4 suite citing CON-18;
  - the run record holding the cited IDs and labelled `mock-gated`;
  - the cited trace loading.

  GATE-10 is the CI run itself. GATE-16 and GATE-4's two process rules (who merges, and no M1 work before the merge) are verified by the maintainer, not by a test. GATE-4 follows CON-16(d): an agent may run the merge on the maintainer's instruction naming the PR.

## Consequences
- The M0 PR is opened, reviewed and fixed by the agent loop and left for the maintainer's merge. No M1 task starts before it. Lab work is not gated.
- A wrong path or an unrecorded run ID in any gate record now fails CI.
- Each later gate's task must write its SPEC 095 section before its record, or the suite refuses the record (GATE-6).
