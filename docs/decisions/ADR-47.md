# ADR-47 — What `docs-inventory` recomputes from a committed twin, and a candidate's bytes

**Status:** accepted (issue #84, checks 1 and 2; Class A, `crates/xtask`). **IDs affected:** P16-20, P16-21, LOOP-12, HYP-5, HYP-22, GATE-23.

## Context
The review of PR 82 found by mutation that `cargo xtask docs-inventory --check` accepted a committed twin whose `within` flag had been flipped, and a candidate hypothesis edited after its loop (issue #84). GATE-23 reads those flags, and `lab/` is outside `env-hash`, so neither change was caught anywhere.

Two things had to be read into the files to check them.

**A `null` figure has two meanings.** `acn loop twin` writes each twinned quantity as `tolerance`, `treatment`, `control`, `effect` and `within`. It writes `null` both for a divergence that is undefined and for an `effect` the falsifier never reads. The frozen code decides `within` from the typed values, where the two differ; the JSON alone does not say which a `null` effect is.

**A candidate is free to edit (CON-23), but a committed loop names its hash.** The report records `hypothesis.path` and `hypothesis.hash`, the BLAKE3 of the file's bytes (HYP-5).

## Decision
- **`within` is recomputed for every twinned quantity** and must equal what is written. A quantity is within when every figure is defined and at or under its tolerance, the bound included, as in `crates/acn-hyp`.
  - A `null` `treatment` or `control` is undefined: a twinned cell has both arms live (HYP-22), so the figure was computed.
  - A `null` `effect` is undefined when the same twin has a defined `effect` for that quantity in another cell, which shows the falsifier reads it. Otherwise it is an effect that is not read, and does not count.
  - An entry of any other shape, a missing figure included, is refused.
- **A candidate that is still in the tree must have the bytes its committed loops ran.** When `hypothesis.status` is `candidate` and `hypothesis.path` names a file in the repository, its BLAKE3 must equal `hypothesis.hash`. A path that leaves the repository is refused. A candidate that is no longer in the tree is not checked.
- **Frozen files are not checked here.** `env-hash` covers them, and an `env-change` amendment after a loop is legitimate (CON-7); the old report keeps the hash it ran.
- **The code in `crates/acn-hyp` is not called.** The check reads the committed JSON only, so it does not depend on the frozen crate's types staying as they are.

## Consequences
- **A candidate with a committed loop is pinned.** To iterate on it, copy it to a new file, or loop the edited file again and commit that loop in place of the old one. This is the cost of citing a loop from a gate.
- **One case stays ambiguous.** A quantity whose effect the falsifier reads, but which is undefined in every twinned cell of a twin, is read as "not read". The flag is then expected to be what the arm figures give. No committed twin is in this case, and issue #81's HYP-22 change is the place to make a twin say which quantities it reads.
- **Not done here** (they stay on issue #84): a twin and its L2 verdict removed together, since a report does not name its twins; a verdict field that the `verdict_id` does not cover; and reading the twin JSON in the GATE-23 test, which lands with the M1 record.
