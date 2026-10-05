# ADR-24 — T05b.2: `acn evidence verify` and the gates between layers

**Status:** accepted (T05b.2, Class C: `crates/acn-hyp`). **IDs affected:** LOOP-1, LOOP-2, LOOP-4; HYP-15, HYP-20, HYP-22; CON-8, CON-31.

## Context
T05b.1 (ADR-23) built the L1 loop and its regeneration. T05b.2 adds:
- the chain walk of LOOP-2;
- the verify half of LOOP-1;
- the two gates of LOOP-4, which the `twin` (T11b) and `promote` (T30) commands will call.

No L2 or L3 object exists yet, so only L1 chains can be walked today.

## Decision

### `acn evidence verify <id>` (LOOP-2, LOOP-1)
- **Resolving the id.** The id is a loop_id when `runs/loop/<id>/` exists. Otherwise it is a verdict_id, and every loop report whose final verdict it is gets walked, in loop_id order. Two loops can end on one bundle set (LOOP-10(c)), and each such chain must hold.
  - An id nothing names gives the finding `no_loop_report`.
  - A loop directory whose report cannot be read or parsed gives `report_refused`, rather than being skipped, since it might have named the verdict.
  - A string that is not 64 lowercase hex digits is refused outright with `bad_id`.
  - A directory `runs/loop/<id>/` makes the id a loop_id, so a report planted under a verdict's id blocks verifying by that verdict_id. That can only make a chain fail, never pass.
- **Read from `runs/` itself.** `runs` is made canonical first. A loop directory, or a `report.json`, that is a symbolic link is refused with `report_refused`, and `--from-report` refuses one too. Otherwise the walk would check one tree and regenerate into another.
- **Malformed reports.**
  - A report missing a field the walk needs (`verdict_id`, `build_hash`, `engine_hash`, `hypothesis.path` or `hypothesis.hash`), with a `verdict_id` that is not an id, or with a hypothesis path that leaves the base, gives `report_refused`. Nothing is guessed.
  - A listed bundle without a `run_id` and `bundle_digest` gives `report_refused` too.
- **Findings are collected, not raised.** Each broken link becomes a finding with its code, so one run reports every link that fails. The command's `ok` is true iff there is no finding and at least one chain was walked (CON-8).
- **Links per report:**

  | Link | What is checked | Finding |
  |---|---|---|
  | Recorded layer | equals the derived one: a report is L1 (LOOP-1) | `layer_mismatch` |
  | Each listed bundle | verifies with its views; `bundle_digest` is the recorded one | `bundle_invalid` |
  | Each listed bundle's layer | is L1, since an L1 report rests only on L1 bundles | `layer_mismatch` |
  | Hypothesis file | still has the recorded hash; otherwise the walk of this chain stops here, so the change is reported once | `hypothesis_changed` |
  | Final verdict | recomputed from those bundles by `verdict::verdict`, the function `acn hyp verdict` uses (HYP-20); has the recorded verdict_id and the bytes of `runs/verdicts/<verdict_id>/verdict.json`. `verdicts` counts the verdicts actually recomputed | `verdict_mismatch` |
  | Final verdict's layer | is L1 | `layer_mismatch` |
  | The report | regenerates as LOOP-14 says, into `runs/regen/<loop_id>/<n>/` | `not_regenerated`, or the regeneration's own code |

- **Another build or engine.** A report made by another build or engine gives the single finding `not_regenerable_with_this_build`, after the bundle checks, which need no particular build. The verdict recomputation and the regeneration are skipped, since neither can succeed on another binary (CON-31). Evidence is therefore verified on the build that made it.
- **Engine as well as build.** LOOP-2 names the build. The engine is required too, because a verdict refuses bundles of another engine (HYP-20) and `run_id` includes it (CON-29), so nothing can be recomputed or regenerated across engines.
- **Verify writes.** It writes only what a regeneration writes, under `runs/regen/`. A verdict_id named by *n* loops regenerates each of them, so the cost is *n* full loops.

### The gates (LOOP-4)
- **`evidence::twin_gate(report)`** regenerates the report. An L1 report that does not regenerate byte-identically is refused with `twin_refused`, and the regeneration's own reason is quoted, including another build.
- **`evidence::promote_gate(h, l1, l2)`** opens only when every one of these holds, and otherwise refuses with `promote_refused`:
  - `l1` and `l2` are verdicts of `h`: each verdict_id recomputes from `h`'s hash and the verdict's bundles (HYP-15), so a copy of the file with a looser tolerance cannot stand in;
  - `l1` is an L1 verdict;
  - `l2` exists and is L2, meaning it holds live twins on the mock (LOOP-1);
  - `l2`'s sim bundles are exactly `l1`'s, so the twin is of the verdict being promoted and its divergences are measured against the same sim sample;
  - neither `l2`'s file-level reasons nor any slice's include `twin_failed`;
  - every decision cell of `l1` (HYP-22) is twinned in `l2`.

  **Matching decision cells.** A decision cell is matched by slice key and cell key, because the two verdicts index their cells separately.
  - Leaving out a decision cell's live treatment already makes `l2` record `twin_failed`.
  - Leaving out every live arm of a slice records only `partially-twinned`, and the gate still refuses it.
  - A decision-cell index with no cell counts as untwinned: the gate fails closed.
- **What the caller checks.** The gate takes verdicts in memory. The `promote` command (T30) must first run `evidence::verify` on the L1 report and recompute the L2 verdict from its bundles, so that "passed the verifier of the layer below" (LOOP-4) holds for what it is given.
- **The waiver.** `twin_required = false` waives the L3 gate. LOOP-4 permits that only for network-free hypotheses such as POC 4. No machine check can tell whether a hypothesis is network-free, so the waiver stays a review item for the freeze PR (HYP-26).

## Consequences
- LOOP-1, LOOP-2 and LOOP-4 are in scope, so SPEC 085 §1 and §2 are implemented apart from LOOP-12's commands (T11b, T30), LOOP-20/21 and §4 (T07).
- Once L2 and L3 objects exist, `verify` walks their `derived_from` links as well.
