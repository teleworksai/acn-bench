# ADR-22 — SPEC 085 v0.2: the choices the review of the loop spec left open

**Status:** accepted (T05b, `spec-change`; the maintainer delegated these choices). **IDs affected:** LOOP-1, LOOP-2, LOOP-4, LOOP-10 to LOOP-15; HYP-8, HYP-9, HYP-20, HYP-22; CON-18, CON-31; HAR-10, HAR-50.

## Context
The review of SPEC 085 v0.2 (PR #17) raised seven questions the spec could answer more than one way. The maintainer asked for the recommended answer to each. This ADR records them so that T05b implements one reading.

## Decision
1. **Builds.** Regeneration and `acn evidence verify` run only on a binary whose `build_hash` equals the recorded one. On any other build they report `not_regenerable_with_this_build` with `ok: false`.
   - CON-31 claims byte identity only within one build. A non-failing status would let a gate pass on evidence nobody regenerated.
   - A loop reuses an existing bundle only from the same build and engine, and otherwise aborts with `build_mismatch`.
   - `loop_id` excludes the build. After a code change, the same inputs therefore hit the existing loop directory (`loop_exists`) and the old bundles (`build_mismatch`). Re-running means starting from a `runs/` without them, since `runs/` is not versioned.
2. **Workload as a parameter.** A file that varies `workload` takes `--workload <value>=<file>` for every value, and each cell runs on its value's file.
   - HAR-10 makes a non-knob `vary.<name>` change nothing in the harness. Without the map, POC 4's `workload` cells would all run on one file under three labels.
   - The map enters `loop_id` and the report by hash.
3. **`bisect` is deferred.** Holding the other parameters at the control's values switched the treatment off. Choosing the levels, the midpoint, termination and a restricted boundary language is a design question tied to SPEC 080 §6 question 2.
   - T05b ships `grid` and `random`, and a file whose `search` is `bisect` is refused.
   - `lab/hypotheses/p17-a2a.toml` is a candidate using it, so it cannot be looped until `bisect` is specified.
4. **L2 and L3 output are separate objects** under `runs/loop/<loop_id>/twin/` and `promote/<provider>/`, each with its own `layer` and `derived_from`. `report.json` stays L1 and immutable, so it still regenerates byte for byte, and no object is produced by two layers.
5. **A file with no `[control]` is refused** (CON-18). HYP-8 lets a candidate omit its control so that it can be linted while it is drafted, but a loop over it would measure nothing.
6. **Mock profile per provider.** When the file varies `provider`, `--model` may map each value to a mock profile. p4 expects different cache mechanics per provider (MLM-50), and HYP-20 already allows different models across slices. A pinned file is checked against `pins.models` under the `provider` value, as `verdict.rs` does.
7. **Budget.** A reused bundle counts exactly as a made one. The loop stops at the first batch that does not fit, and the report does not record which bundles were reused, so a regeneration (which reuses none) writes the same bytes.
   - A budget smaller than the first batch is refused.
   - A frozen file's budget must cover its whole grid, because a frozen verdict reads every cell.

### Readings made while revising
- **`netem` is L3 on either backend**, as the layer table's L3 row lists it. A verdict's layer is the highest among its bundles, so sim plus live twin is L2.
- **`random`'s float sampler** is written out in LOOP-10(b) (`min + (max − min) × u`, with `u` built from the top 53 bits of one output), because the workspace pins `rand_core` and `rand_chacha` but no `rand` distribution crate. The integer and index draws use ADR-19's ranged sampler.
- **A verdict no loop report names** fails `evidence verify` with `no_loop_report`. `acn hyp verdict` alone records no workload path, so such a verdict cannot be regenerated, and only loop output is citable (LOOP-30).
- **The executor receives HAR-50's inputs**, not a seed, because HAR-50 accepts a seed only for hypothesis `none`. The runner computes the expected run_id itself and checks each manifest.
- **The lab-note draft's "suggested next layer"** is left to T05b. A reasonable rule is L2 when `twin_required` holds, and otherwise L3.

## Consequences
- T05b implements LOOP-1, 2, 4, 10, 11, 13, 14 and 15 under this reading, with known-answer vectors for `loop_id` and `random`.
- `bisect` needs a further `spec-change`.
- SPEC 080 §6 question 5 now points to LOOP-10(d).
