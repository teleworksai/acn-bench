# ADR-37 — T11b: the L2 twin, in two PRs

**Status:** accepted (T11b.1; `spec-change`, SPEC 085 Draft v0.4). **IDs affected:** LOOP-1, LOOP-2, LOOP-4, LOOP-12, LOOP-15, LOOP-16; HYP-20, HYP-22; CON-7, CON-8, CON-25, CON-26.

## Context
T11b makes `acn loop twin` run L2: the decision-relevant cells of a loop report run in `live` on the mock, and the divergence from `sim` is recorded (CON-25). SPEC 085 v0.3 named the command but left its object's format "to land with T11b". It also left open which cells the *k* best and worst are, what the live runs use as inputs, and how `acn evidence verify` walks an L2 object.

LOOP-15 puts every loop decision in `acn-hyp`, which is in the frozen set. Choosing the cells, the verdict, and the twin object's every word are such decisions. The `acn evidence verify` chain is in `acn-hyp` too.

## Decision
- **Two PRs.**
  - T11b.1 is this `spec-change` (SPEC 085 v0.4: LOOP-12 becomes a MUST with its steps; LOOP-16 is new).
  - T11b.2 implements it in `crates/acn-hyp` (cell choice, the run of the gate, the verdict, the twin object, verify), with a live executor in `acn-cli`. It is a Class C `env-change` PR: the maintainer merges it, after the adversarial review of CON-7.
- **Which cells.** The decision cells of every slice, then the *k* best and *k* worst cells by LOOP-11's ranking. With `k = 1` by default, the report's own best and worst, the cells a reader quotes, are always twinned. `--top 0` gives the minimum that LOOP-4's L3 gate needs.
- **The live runs repeat the L1 inputs exactly.** The same seeds pair the replicates by index (HYP-22), and the same `scenario_hash` is required by HYP-20. With no scenario in `acn loop run` yet (ADR-34), the twin compares the mock's virtual timing in `sim` with its served timing on the wall clock. That twin compares the mock with itself, which is what L2 claims about the simulator.
- **`acn loop twin` serves the mock itself** on the loopback interface, from the embedded profiles. There is no `acn mock serve`, and an external endpoint would make the twin's backend something other than the mock (CON-26). The mock server's seed is the hypothesis's run seed (HYP-9), so the served mock is fixed by the hypothesis, as the sim one is.
- **Where the object goes.** Each twin gets `twin/<verdict_id>/`, named by its L2 verdict's id. A live run is a measurement, not a regeneration, so two twins of one loop on different bundles are two objects, and neither overwrites the other.
  - The same live bundles give the same verdict_id, so a repeat on the same runs directory fails with `twin_exists` rather than rewriting.
  - Measuring again uses another runs directory.
- **`ok` is completion.** As for `acn hyp verdict` (HYP-20), a divergent twin completes: it is recorded as `twin_failed` and stops L3 at LOOP-4's gate.
- **Verify.** A twin object verifies by its links: each live bundle verifies and twins an L1 bundle of the report, and the L2 verdict recomputes. Live bundles are not regenerated: their chain ends in the L1 bundles they twin (LOOP-2).

## Consequences
- T11b.2 is the first post-M0 change to the frozen set. Until the maintainer merges it, `acn loop twin` does not exist, and the M1 tasks that do not need it (T13 onward) proceed.
- The L3 gate (`promote_gate`, LOOP-4) already reads an L2 verdict's twinned decision cells; T11b.2 produces those verdicts.
- With scenarios in `acn loop run` (an env-change, ADR-34), the twin carries the scenario into live through the proxy of SPEC 020 §5 without a change to this format.
