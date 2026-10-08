# ADR-45 — An amendment may keep a frozen file unpinned (HYP-26)

**Status:** accepted (issue #70, PR 72: spec-change and env-change). **IDs affected:** HYP-26, HYP-23.

## Context
PR 72 adds `tensormesh` to `hypotheses/p4.toml` (ADR-44), and its `pr-check` failed rule HYP-26: every changed `real-api` file under `hypotheses/` must carry `[design].pins`.

`p4.toml` has been frozen without pins since M0. Pinning it now would make it refuse its own mock runs, because `pins.models` cannot name a file's only backend (SPEC 100 §7 q3, issue #18). No label clears a HYP-26 finding. The check was written for the freezing PR, yet it also bound every later change to an already frozen, unpinned file.

The maintainer chose on 2026-10-08 to narrow the rule rather than resolve #18 first, or park the provider.

## Decision
- **SPEC 080 v0.2, HYP-26.** An *amendment* is a Class C PR that changes a file frozen without `[design].pins` at the PR's merge base. An amendment may leave the file unpinned, and its verdicts keep `unpinned-inputs` (HYP-23) until a PR pins it.
- **A PR must not remove `[design].pins`** from a frozen file.
- **Without a merge base**, as in `pr-check`'s path-list mode, nothing counts as an amendment.
- **`cargo xtask pr-check`** reads the file at the merge base. The changed file is exempt from the pins finding only if, at the merge base, it was a `real-api` file with no `pins` key that the base's `env-hash.json` records with those exact bytes. A missing, unrecorded, unparsable, mock-only or pinned base file does not exempt it.
- **No unpinning.** A file that had a `pins` key at the merge base and has no pins at the head is a finding, whatever its backends. The other freeze checks are unchanged: it must load, load as frozen, lint, and match the head commit.

## Consequences
- An amended file stays visibly uncitable for real providers: `unpinned-inputs` is on every verdict, so nothing is weakened for citation.
- The exemption follows the path, not the content: an amendment to `p4.toml` may change its statement, predicate or parameters and still stay unpinned. That is acceptable only because `unpinned-inputs` blocks citation (`crates/acn-hyp/src/verdict.rs`, `crates/xtask/src/evidence.rs`). Each amendment is still a Class C PR with its adversarial review.
- Freezing a new `real-api` file still requires pins, as before, and so does adding `real-api` to a mock-only frozen file.
- A file cannot be unpinned by any PR.
- `crates/xtask/tests/freeze.rs` gains `an_amendment_may_keep_a_frozen_file_unpinned_but_never_unpin_one`.
- `xtask` is outside the frozen set, so `engine_hash` does not move for this part.
