# ADR-55 — SPEC 080 v0.4: a file with no real backend pins no model

**Status:** accepted (`spec-change`; issue #102; follows ADR-54). **IDs affected:** HYP-9; by reference HYP-20, CON-13, CON-14, CON-26.

## Context
HYP-9 in v0.3 said that a file with no `provider` parameter MUST have exactly one key in `pins.models`, a backend other than `mockllm`, and made no exception. A file whose `backends` do not include `real-api` has no such backend. Read to the letter, it had to name one real backend and a model for it, and nothing would ever have been checked against that key, since HYP-20 looks a model up for real-backend bundles only. The code merged with ADR-54 required the table to be empty in such a file. The adversarial review of that change found the reading to be against the letter of the MUST, and it was raised as `spec-conflict: HYP-9` (issue #102). The maintainer chose the first of the issue's three options: amend the text.

## Decision
SPEC 080 becomes v0.4. HYP-9 now states the two cases of a file with no `provider` parameter:
- its `backends` include `real-api`: `models` has exactly one key, a backend other than `mockllm`, as in v0.3;
- its `backends` do not include `real-api`: `models` is empty.

No requirement ID is added or removed. No code changes: `pins_without_provider` in `crates/acn-hyp/src/file.rs` already does this, and `crates/acn-hyp/tests/format.rs` and `crates/acn-hyp/tests/verdict_semantics.rs` already hold both cases.

The options not taken: keeping the text and making such a file name a backend that is never checked, which pins nothing; and refusing `pins` altogether in such a file, which would stop a mock-only file pinning its scenario and workload.

## Consequences
- **The frozen set is untouched**, so `engine_hash` and `env-hash.json` do not change.
- **Only mock-gated files are concerned.** Their verdicts are never cited (CON-26). `hypotheses/p4.toml` has a `provider` parameter and no `pins`.
- **Issue #102 closes with this PR.**
