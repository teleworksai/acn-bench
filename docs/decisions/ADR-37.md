# ADR-37 — T11b: the L2 twin, in two PRs

**Status:** accepted (T11b.1; `spec-change`, SPEC 085 Draft v0.4, SPEC 040 Draft v0.4). **IDs affected:** LOOP-1, LOOP-2, LOOP-4, LOOP-12, LOOP-15, LOOP-16; HAR-23, HAR-25, HAR-26; HYP-20, HYP-22; CON-5, CON-7, CON-8, CON-25, CON-26, CON-29, CON-30.

## Context
T11b makes `acn loop twin` run L2: the decision-relevant cells of a loop report run in `live` on the mock, and the divergence from `sim` is recorded (CON-25). SPEC 085 v0.3 named the command but left its object's format "to land with T11b". It also left open which cells the *k* best and worst are, what the live runs use as inputs, and how `acn evidence verify` walks an L2 object.

LOOP-15 puts every loop decision in `acn-hyp`, which is in the frozen set. Choosing the cells, the verdict, and the twin object's every word are such decisions. The `acn evidence verify` chain is in `acn-hyp` too.

## Decision
- **Three PRs.**
  - T11b.1 is this `spec-change`. In SPEC 085 v0.4, LOOP-12 becomes a MUST with its steps, LOOP-16 is new, and LOOP-15 lists the mode and endpoint among the executor's inputs. In SPEC 040 v0.4, HAR-26 is new.
  - T11b.2 implements HAR-26 in `acn-harness`, a Class B PR.
  - T11b.3 implements LOOP-12 and LOOP-16 in `crates/acn-hyp`: the cell choice, the gate, the verdict, the twin object and verify. A live executor goes in `acn-cli`. It is a Class C `env-change` PR: the maintainer merges it, after the adversarial review of CON-7.
- **Which cells.** The decision cells of every slice, plus the *k* best and *k* worst cells. The ranking extends LOOP-11's single best and worst to a full ranking with the same ties, so its first entries are the report's own. With `k = 1` by default, the cells a reader quotes are always twinned. `--top 0` gives the minimum that LOOP-4's L3 gate needs.
- **The live runs repeat the L1 inputs exactly.** The same seeds pair the replicates by index (HYP-22), and the same `scenario_hash` is required by HYP-20. With no scenario in `acn loop run` yet (ADR-34), the twin compares the mock's virtual timing in `sim` with its served timing on the wall clock. That twin compares the mock with itself, which is what L2 claims about the simulator.
- **The harness serves the mock, per replicate (HAR-26).** The review of the first draft found two faults in one mock server for the whole twin:
  - its ephemeral port entered `opt.endpoint`, and so every live run_id;
  - one mock shared its RNG, caches and slots across replicates and arms, which a `sim` run never does, so caching hypotheses would diverge by construction.

  The reserved endpoint `acn-mock://loopback` instead makes the harness serve a fresh mock per replicate, from the replicate's seed, as in `sim`, and as ADR-36's proxy is per replicate. The run_id is then a function of the inputs, and verify can recompute it. An external endpoint is not used, because it would make the twin's backend something other than the mock (CON-26). There is no `acn mock serve`.
- **Where the bundles and the object go.** A live run is a measurement, not a regeneration, so it is never reused.
  - Each twin runs into `runs/live/<n>/` and records that path, so verify finds its bundles under `runs/`.
  - Its object goes to `twin/<verdict_id>/`, named by its L2 verdict's id.
  - Two twins of one loop are two objects, and neither overwrites the other. An earlier `--runs-dir` option was dropped, because it put bundles where verify could not find them.
- **An existing L2 verdict.** One that is byte-identical is kept, and one that differs fails with `verdict_conflict`, as LOOP-10(c) treats verdicts shared between loops.
- **`ok` is completion.** As for `acn hyp verdict` (HYP-20), a divergent twin completes. When the hypothesis requires a twin, the divergence is recorded as `twin_failed` and stops L3 at LOOP-4's gate. Otherwise only the divergences are recorded.
- **Verify.** A twin object verifies by recomputing everything that can be recomputed:
  - the cell choice, from the report and `top`;
  - each live run_id, from its L1 bundle's inputs;
  - the L2 verdict's bundle set and its bytes;
  - `twin.json` itself.

  Live bundles are verified but not re-run: their chain ends in the L1 bundles they twin (LOOP-2).

## Consequences
- T11b.3 is the first post-M0 change to the frozen set. Until the maintainer merges it, `acn loop twin` does not exist, and the M1 tasks that do not need it (T13 onward) proceed.
- A twin re-runs the whole L1 loop for its gate. It costs at least as much as its loop, plus the live runs on the wall clock.
- The L3 gate (`promote_gate`, LOOP-4) already reads an L2 verdict's twinned decision cells; T11b.2 produces those verdicts.
- With scenarios in `acn loop run` (an env-change, ADR-34), the twin carries the scenario into live through the proxy of SPEC 020 §5 without a change to this format.
