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
  - A `null` `effect` is undefined when the same twin has a defined `effect` for that quantity in another cell, which shows the falsifier reads it.
  - Where no cell shows that, the file does not say which `null` it is, and the flag is accepted as written. A written `false` can only fail a gate, never pass one.
  - An entry of any other shape, a missing figure included, is refused.
- **Figures are parsed from the text as written.** `serde_json`, as this workspace builds it, may read a float one unit in the last place off. A truthful figure one step over its tolerance would then be refused. The standard library's parser is correctly rounded.
- **A file outside `hypotheses/` that is still in the tree must have the bytes its committed loops ran.** Its BLAKE3 must equal `hypothesis.hash`. Which files are compared goes by where the file is (HYP-3), not by the `status` the report gives itself:
  - a report must give a `status` of `candidate` or `frozen` and a `path` inside the repository;
  - `frozen` with a path outside `hypotheses/` is refused;
  - a path that is a directory or a symbolic link is refused, and a link's target is never read.
- **Files under `hypotheses/` are not compared here.** `env-hash` covers them, and an `env-change` amendment after a loop is legitimate (CON-7); the old report keeps the hash it ran.
- **A candidate that has left the tree is not compared by `docs-inventory`.** A test on this repository's own tree requires every committed loop's candidate to be present with the bytes it ran, so moving one away fails CI here.
- **The code in `crates/acn-hyp` is not called.** The check reads the committed JSON only, so it does not depend on the frozen crate's types staying as they are.

## Consequences
- **A candidate with a committed loop is pinned.** To iterate on it, copy it to a new file, or loop the edited file again and commit that loop in place of the old one. This is the cost of citing a loop from a gate.
- **One case stays unchecked.** A quantity whose effect the falsifier reads, but which is undefined in every twinned cell of a twin, cannot be told from one whose effect is not read. Either flag is accepted for it. It needs incomplete replicate pairs to arise, and no committed twin is in it. Issue #81's HYP-22 change is the place to make a twin say which quantities it reads.
- **A flipped flag is not caught where the file is silent.** For a quantity with a `null` effect in every cell and arm figures inside the tolerance, `true` rewritten as `false` passes. On the loop of record that is `cached_token_ratio` and `ttft_p50_ms`; `cost_per_success` is fully checked. The reverse edit, `false` to `true` over an out-of-tolerance or undefined arm, is always caught.
- **Not done here** (they stay on issue #84): a twin and its L2 verdict removed together, since a report does not name its twins; a verdict field that the `verdict_id` does not cover; and reading the twin JSON in the GATE-23 test, which lands with the M1 record.
