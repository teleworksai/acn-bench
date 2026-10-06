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

## T11b.2 notes
- The served mock reuses `acn_mockllm::server::router`. When the replicate ends, after its proxy if it has one, its server shuts down gracefully:
  - it stops listening;
  - it closes idle connections;
  - it waits for any request still being answered, such as a timed-out attempt's late response, which is bounded by the mock's own response time.

  A replicate that fails first drops its server, which stops at once. The run's one HTTP client serves every replicate. A pooled connection to an earlier replicate's server has been closed by then, so it is never reused.
- HAR-23's probe runs against each replicate's server before its sessions. `GET /v1/models` reads only the profiles, so the probe leaves the mock as `sim` would build it.

## T11b.3 notes
- **T11b.3 is two `env-change` PRs.** Planned, it is about 2,000 lines of the frozen set, too much to review adversarially at once.
  - T11b.3a changes no behaviour:
    - `Request` gains `mode` and `endpoint` (LOOP-15), and both executors pass them through;
    - the loop runner computes and checks a bundle's run_id in either mode;
    - `read_report` and `write_or_keep` are extracted from `regenerate` and `run`;
    - LOOP-11's single best and worst become a full ranking;
    - `loop_twin::choose` picks LOOP-12's cells;
    - the twin checks of `promote_gate` move to `loop_twin`.
  - T11b.3b adds `acn loop twin`, the twin object and its walk in `acn evidence verify`. It ships the command and its verification together, so that no merged state holds twin objects that verify does not walk (LOOP-16).
- **The ranking.** It reads the effects of the recomputed L1 verdict, the values the report records. It sorts stably by value, so equal effects keep slice-key and HYP-14 order, and −0 equals +0. The first best and worst are therefore exactly the report's, and LOOP-11's bytes are unchanged; a test checks both.
- **A live run_id.** It is the L1 computation with `mode = live` and the one option `opt.endpoint = acn-mock://loopback` (HAR-26). Every other option is at its default and so is dropped from the parameters (CON-29).
- **Codes.** `twin_exists` and `twin_mismatch` are declared in T11b.3a with the rest of LOOP-12's codes, and are first produced by T11b.3b.
- **What `choose` refuses.** It takes only an L1 verdict of its own hypothesis (`twin_refused`), as `promote_gate` does, because the choice is the loop runner's (LOOP-15) and must not rank another file's effects. A decision cell outside its slice is an internal error, not a cell quietly left out.
- **Links.** A loop report is refused when the report, its loop directory, `runs/loop` or `runs/` is a symbolic link (HYP-4). Before, only the first two were checked.

## T11b.3b notes
- **An arm with no L1 bundle.** A decision cell a budget never ran, or a cell whose control the report lacks, records `derived_from`, `run_id` and `bundle_digest` as `null` for that arm. Nothing is run for it, and the cell is then not twinned (`twinned` is false). A choice that maps to no L1 bundle at all is refused with `nothing_to_twin`.
- **Which cells the divergence block lists.** It lists the cells the L2 verdict marks twinned, with every quantity that has a tolerance. The numbers are the verdict's own divergences, and `within` is the verdict's own test (`Divergence::within`). `verdict.json` does not record `within`, so it is applied again rather than copied.
- **Where the twin runs.** `acn loop twin` takes no `--runs-dir`; it always uses `<root>/runs`, as LOOP-12's signature says.
- **`top` and the hashes.** `top` is a `u32`. The engine and build hashes in the object are the live bundles' own, which `make` has checked against the binary, so the object regenerates from its bundles.
- **Inputs during the twin.** They are checked before every live run and before each write (LOOP-13).
- **What a twin leaves behind.**
  - An aborted twin leaves its `runs/live/<n>/`, which nothing names, and the next twin takes `n + 1`.
  - The gate leaves a `runs/regen/<loop_id>/<n>/`.
- **The verify walk.**
  - It walks a loop's twin objects only when the L1 walk reached the final verdict; an L1 finding already fails the chain.
  - It resolves an L2 verdict_id to every loop with a twin object of that name.
- **Where the L2 chain tests live.** They are in `crates/acn-hyp/tests/loop_twin.rs`, beside the twin tests, rather than in `tests/accept/evidence_chain.rs` as SPEC 085 §5 lists. They need live runs on fast mock profiles, which the acceptance tier's executor (the CLI's, on the embedded profiles) does not offer.
- **After review (PR 48).**
  - **The report read after the gate.** The twin reads the report again after the gate, and it must be byte-identical to the regenerated one, or the twin is refused. A report swapped between the gate and the read could otherwise send a twin's live runs against bundles that never regenerated.
  - **Writes.** The inputs are checked once before the two writes. Only I/O, or a race on the object's existence check, can then leave an L2 `verdict.json` that no twin object names. `acn evidence verify` of that verdict_id reports `no_loop_report`; the verdict is a correct one, and a later twin of the same live bundles keeps it.
  - **What verify fails on.** Nothing under `twin/` or `live/<n>/` is skipped silently. It fails on any of these:
    - an entry under `twin/` that is not a verdict_id, other than a crash's staging directory;
    - an unreadable `twin/`;
    - a link at `twin/`, `runs/live`, `live/<n>` or a live bundle;
    - a directory in `live/<n>` that the twin does not run;
    - a second twin object claiming the same `live_dir`;
    - a live bundle whose build or engine is not the report's (CON-31).
