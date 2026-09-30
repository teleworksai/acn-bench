# acn-bench — development plan

**Status:** Draft v0.2, September 2026 (v0.2: per-provider POC 4, sim↔live twin rule, turn-native transport and A2A lab spikes first, traces from day one, hardware prerequisites, WG cadence). **Owner:** Young He.
**What this is:** the plan for an independent Rust project that builds the ACN experimental substrate (report Appendix E, §E.2) and runs the POC catalogue (POC 1a–16) — and, just as much, a place to find the POCs the catalogue doesn't have yet. ACN is an exploration of a new field. Spec-driven development and testing are the method for the part of the project that produces defensible numbers; they are deliberately *not* applied to the part that produces questions. Two tracks (§1a). No Python in the run path; no required dependency on dora-rs, Genesis or any robotics stack.

---

## 1a. Two tracks

| | Exploration track — `lab/` | Substrate track — `crates/`, `specs/`, `hypotheses/` |
|---|---|---|
| Purpose | find the right questions; try things | produce numbers that survive review |
| Rules | Rust; fmt + clippy; a lab note | full constitution: specs, IDs, tests, controls, determinism, frozen set |
| Dependencies | anything, including dora/ROS/partner stacks | substrate must run without them; adapters optional |
| Hypotheses | candidates, editable by anyone incl. loops | frozen, Class C to change |
| Output | `docs/lab/*.md`, exploratory numbers | bundles with run IDs, verdicts, gate evidence |
| Graduation | when someone wants to cite a number → spec-change PR, promote hypothesis, tests, control | — |

The catalogue (POC 1a–16) is the current map, not the boundary. Every milestone in TASKS.md carries an open exploration slot, and new POCs are expected to come out of `lab/`.

## 1. Goals and non-goals

**Goals.** (1) A deterministic, purpose-built ACN simulator/emulator plus workload generator, replayer and control plane — the four substrate parts of Appendix E — as a Cargo workspace. (2) Every POC in the catalogue expressed as a frozen hypothesis file with a machine-checkable falsifier and a mandatory control, so that a run produces a verdict, not an opinion. (3) Reproducibility as a property of the build: same seed + same scenario + same environment hash ⇒ same bundle in `sim` mode, bit for bit; statistically reproducible in `live` mode. (4) A kit that a second group can run (POC 16).

**Non-goals.** A general network simulator (ns-3 exists); a robotics stack; a production router or proxy; a GUI. Physical radios and real inference nodes are external to the workspace and enter only through the control plane.

## 2. Two execution modes, one code path

The emulator has a `sim` mode and a `live` mode behind the same `LinkModel` and `Clock` traits.

- **`sim`** — a discrete-event simulator. No sockets. Time is a virtual clock advanced by the event queue; every random draw comes from an injected seeded RNG (ChaCha20). A run is bit-identical across machines of the same target and build (CON-31). This is the ACN simulator; it is what unit and acceptance tests run, and what an auto-research loop searches over.
- **`live`** — the same link models applied by a userspace impairment proxy (tokio) between real sockets: the harness or generator on one side, an inference endpoint (mock or real) on the other. Wall clock, real TCP/HTTP behaviour, seeded impairment schedule. Statistically reproducible: the seed, scenario hash and environment hash are recorded in the bundle.
- **`netem`** (Linux, feature-gated, M4) — the same scenario driven into `tc`/netem inside network namespaces, used to validate that `live` matches kernel-level impairment on the traces that matter.

Mac arm64 is the primary development platform, so `sim` and `live` are first-class from M0 and `netem` is a later validation path. This ordering is also the honest one: the credibility of the emulator rests on its impairment traces being measured (§E.2), not on which layer applies them.

## 3. Workspace layout and crates

```
acn-bench/
  CLAUDE.md               agent contract (this repo's rules)
  PLAN.md                 this file
  TASKS.md                implementation order + kickoff prompts
  Cargo.toml              workspace
  rust-toolchain.toml     pinned stable
  specs/                  numbered specs with MUST IDs (read-only for agents)
  hypotheses/             one TOML per POC: hypothesis, varies, measures, control, falsifier  [frozen]
  scenarios/synthetic/    scenario files (TOML): link models, schedules
  scenarios/measured/     recorded impairment traces (Parquet) + provenance          [frozen]
  crates/
    acn-trace     §3.5 trace schema, Arrow/Parquet IO, run bundle + manifest      [frozen: schema]
    acn-emu       link models, scenario loader, sim engine, live proxy, (netem)
    acn-mockllm   deterministic mock inference server: prefill/decode timing, cache accounting
    acn-harness   minimal agent harness with switchable discipline knobs (POC 4), real or mock backend
    acn-gen       workload generator: sessions/turns/calls per Appendix C parameters, seeded
    acn-replay    environment replayer (recorded sensor streams → remote planner stub) + toy loop
    acn-ctl       control & evidence plane: HTTP/JSON API, run registry, bundle collection
    acn-hyp       hypotheses registry + verdict tool                               [frozen]
    acn-attrib    attribution & analysis: tail decomposition, network share, heatmaps [frozen: core]
    acn-cli       the `acn` binary (subcommands over the crates above)
    xtask         trace-check, docs-inventory, env-hash, ci
  tests/accept/   spec-acceptance suites, one per POC, citing requirement IDs
  tools/          ci.sh and small scripts (bash only)
  docs/           guides, decisions/ADR-<n>.md, generated inventory
  runs/           bundles (gitignored)
```

Crate dependency direction (no cycles): `acn-trace` ← everything; `acn-emu` ← `acn-gen`, `acn-harness`, `acn-replay`; `acn-ctl` ← all runtime crates; `acn-hyp`, `acn-attrib` ← `acn-trace` only; `acn-cli` ← all.

## 4. The requirement-ID system

| Prefix | Spec | Covers |
|---|---|---|
| CON | 000 | constitution: platform, toolchain, determinism, layout, gates, classes |
| TRC | 010 | trace schema (§3.5), bundle layout, manifest, hashes |
| EMU | 020 | link models, scenario format, sim engine, live proxy, netem backend |
| MLM | 030 | mock inference server timing model and cache accounting |
| HAR | 040 | harness loop, discipline knobs, cached-token measurement |
| GEN | 050 | workload generator distributions and replay |
| RPL | 060 | environment replayer, scoring, toy closed loop |
| CTL | 070 | control plane API, run registry, bundle collection |
| HYP | 080 | hypothesis file format, verdict semantics, freeze rules |
| ATR | 090 | attribution: tail decomposition, network-attributable share, controls |
| GATE | 095 | milestone gates M0–M4 |
| P4, P1A, P1B, P11, P13, P7, P16, … | 100-series | one spec per POC: experiment protocol, acceptance suite, expected outcome |

Every MUST is numbered (`EMU-7`). Every implemented MUST is cited by at least one test via a `/// Cites: EMU-7, EMU-8` doc comment on the test function; `cargo xtask trace-check` fails CI otherwise. Hypothesis files cite the POC spec they instantiate and the report hypotheses they test (`H-1`, `G13`).

## 5. Milestones

| Gate | Delivers | Exit evidence |
|---|---|---|
| **M0** — bootstrap + first result | workspace, gates, `acn-trace`, `acn-mockllm`, `acn-harness` with knobs, `acn-hyp` (verdict on a bundle), POC 4 acceptance suite, first measured trace (phone-tethered 5G walk) | POC 4 run on the mock backend (labelled *model-of-caching*, not a result) and on **at least two real providers** with cached-token counters; per-provider verdicts; one measured trace with provenance; human sign-off |
| **M1** — substrate MVP (POC 16 minimum) | `acn-emu` sim + live proxy, `acn-gen`, `acn-ctl`, `acn-attrib` core, bundle regeneration from run ID, sim↔live twin test | a scenario run twice in `sim` is bit-identical; the same scenario in `live` agrees within the declared tolerance (CON-25); a second machine regenerates the bundle; kit + POC 4 + one exploratory transport number presented to the WG |
| **M2** — first network results | POC 1a heatmap, POC 1b tail decomposition, POC 11 and POC 13 as extra columns | verdicts for 1a, 1b, 11, 13 with controls; heatmap artifact |
| **M3** — calibration *(hardware-gated, see §11)* | POC 7 against two real inference nodes through the control plane | measured regeneration rate vs the 8–10 GB/s estimate |
| **M4** — Linux validation + trace library | `netem` backend, trace library at three sets (5G handover walk, congested Wi-Fi cell, cross-region WAN), POC 12 and 14 (or their graduated turn-transport successor) | `live` vs `netem` agreement on the measured traces; kit release tag |
| later | `acn-replay` toy loop, POC 3a with a partner, POC 5/6/8/10/15; POC 2a only if real multi-worker inference is available | per POC spec |

Each gate is a spec (095) with an acceptance suite; a gate closes with a human-merged PR that records the evidence in `docs/gates/M<n>.md`.

## 6. Determinism design (CON-5)

- `Clock` trait with `SimClock` (virtual) and `WallClock`; no direct `Instant::now()` or `SystemTime::now()` outside `acn-emu::clock` (clippy `disallowed_methods` enforces it).
- `Rng` injected as `rand_chacha::ChaCha20Rng` seeded from the run seed; sub-streams derived per component by name (`seed_for("emu.link0")`) so adding a component does not shift others.
- Async: tokio with `start_paused = true` in `sim`; in `live`, all impairment schedules are precomputed from the seed before traffic starts, so the schedule is deterministic even if socket timing is not.
- Every bundle carries a `run_id` derived from the seed, the scenario, workload, hypothesis and engine hashes, the mode and the run parameters, encoded exactly as CON-29 specifies; `acn bundle verify` recomputes it. The manifest also carries `build_hash` (CON-31), which names the binary, and its own hash is the `bundle_digest` (TRC-23), which names the data.
- Floating-point reductions in `acn-attrib` use fixed summation order; no parallel reduction without a deterministic reducer.

## 7. Quality gates (CON-9)

```
cargo fmt --all --check
cargo clippy --workspace --all-targets --all-features -- -D warnings
cargo test --workspace                    # unit + integ
cargo xtask trace-check                   # every implemented MUST cited by a test
cargo xtask docs-inventory --check
cargo xtask env-hash --check
cargo deny check                          # licenses + advisories
```

`tools/ci.sh` chains them with `&&`. POC changes add `cargo test -p acn-accept --test <poc>`; Linux emu changes add `cargo test --features netem -- --ignored` on a Linux runner.

## 8. Change classes and the frozen set (CON-7, CON-10)

- **Class A** — docs, tests, tools, scenarios/synthetic: baseline gates.
- **Class B** — crates on the run path (`acn-emu`, `acn-gen`, `acn-harness`, `acn-replay`, `acn-ctl`, `acn-cli`, `acn-mockllm`): baseline gates + affected acceptance suites.
- **Class C** — the frozen set: `hypotheses/`, `scenarios/measured/`, `crates/acn-hyp`, `crates/acn-attrib/src/core`, `crates/acn-trace/src/schema`. Human-merged PR labelled `env-change` with an updated `env-hash`, adversarial review. An auto-research loop MUST NOT have write access to the frozen set (CON-7; HYP-4 for `hypotheses/`).

## 8a. Sim ↔ live twin rule (CON-25)

A deterministic simulator can be a beautiful model of the wrong thing. From M1 on, any scenario whose `sim` result is cited also runs in `live` on the same scenario and seed schedule, and the divergence between the two (per measured quantity, with tolerance declared in the hypothesis file) is computed by `acn hyp verdict`, recorded in the verdict and tracked as a metric in `docs/gates/`. When they drift, the simulator is presumed wrong until shown otherwise. The `netem` backend at M4 extends the same rule to kernel-level impairment.

## 8b. Mock inference is a model, not a result

`acn-mockllm` reproduces whatever cache-accounting rules it is given, so any harness result on it is a test of the harness against our own model of caching. Bundles produced on the mock backend are labelled `backend = "mockllm"` and MUST NOT be cited as results; they gate the suite, they do not answer the question. Real providers differ in mechanics — explicit cache breakpoints (Anthropic `cache_control`), automatic prefix matching (OpenAI), block-granular prefix caching (vLLM / SGLang) — so POC 4 is one harness and one hypothesis instantiated per provider, with a per-provider verdict, and *where the breakpoints go* is itself a discipline knob.

## 8c. The layered, verifiable feedback loop (SPEC 085)

Every POC moves through five loops of increasing time-scale, each verified by a machine check before its output can feed the next, and each allowed to change only artifacts at its own layer or below:

| Layer | Loop | Verifier | May change |
|---|---|---|---|
| L0 Build | edit → gates (seconds) | `tools/ci.sh` | code, tests, docs |
| L1 Sim | hypothesis × parameter ranges in `sim` → verdict (minutes) | bit-identical re-run; control present; ranges respected | parameters, candidate hypotheses, models (with lab note) |
| L2 Twin | same scenario in `live` (hours) | sim↔live divergence within tolerance (CON-25) | simulator calibration only — never the hypothesis |
| L3 Reality | real providers, measured traces, real nodes, `netem` (days) | per-provider verdicts; live↔netem; second-machine regeneration | trace library, cache mappings, mock models |
| L4 Graduation | lab note → human review → spec-change / freeze / cite → WG (weeks) | adversarial review; external reproduction | specs, frozen hypotheses, the catalogue |

Two rules make it verifiable rather than merely iterative. Feedback only flows downward in permission: an L1–L3 result can change code, models and candidate hypotheses, never a frozen hypothesis or a spec; a `fail` on a frozen hypothesis is input to the human layer, not an edit. And evidence chains by hash: every layer's output names the run_ids it was derived from, so `acn evidence verify` walks from a cited number back to regenerable L1 bundles. `acn loop run / twin / promote` drives L1–L3; `docs/evidence/<hypothesis>.md` is the only citation target.

## 9. Where an auto-research loop plugs in

The loop is the first intended user of `lab/`: Claude Code (or any agent) driving lab spikes with lab notes as the only required output is the cheapest way to generate exploration volume, and the graduation review is deliberately the bottleneck. In `lab/`, the loop is unconstrained: it may write candidate hypotheses, spikes and lab notes, and search freely. In the substrate, the loop is a client of `acn-ctl`: it may create runs, choose parameters within the ranges a frozen hypothesis allows, read bundles and verdicts, and open Class A/B PRs; it cannot edit `hypotheses/` or `specs/`. Its own sessions are recorded as a workload (POC 9).

## 10. First two weeks — two tracks in parallel

Substrate: T01–T04 in TASKS.md (bootstrap, trace schema, mock inference server, harness with knobs). The first defensible number is POC 4's per-provider harness-hygiene table, mock-gated by end of week 1, against two real providers by end of week 2.

Lab, starting day one: (1) `lab/turn-transport` — a QUIC (quinn) prototype in which the *turn* is the transport object: one stream per turn with a declared deadline, resumption after a link gap without re-prefill, and the conversation prefix sent as a delta against what the server holds. This collapses POC 11, 12 and 14 into one prototype and is the candidate for a contribution that is more than a measurement of the status quo. (2) `lab/trace-capture` — a small Rust client/server pair that logs RTT, loss and connectivity gaps from a phone tethered on 5G during a walk, producing the first `scenarios/measured/` entry with provenance. (3) `lab/hypotheses/p17-a2a.toml` — a candidate hypothesis for agent-to-agent traffic (two harnesses negotiating over MCP/A2A across the emulated link), the traffic class with no data at all.

## 11. Prerequisites to confirm early

| Need | Decides | Status |
|---|---|---|
| API keys with cache metrics for ≥ 2 providers | M0 exit (per-provider POC 4) | to confirm |
| One phone + 5G plan + a reachable server | first measured trace (M0) | to confirm |
| One GPU node running vLLM or SGLang, reachable by the control plane | M3 (POC 7 regeneration rate); everything in the KV thread leans on this number | **open — decides whether M3 is a quarter or a year away** |
| Multiple inference workers behind a router | POC 2a | not planned; POC 2a deferred until available |
| Linux box with root | M4 `netem` validation | to confirm |
| Robotics / radio partner | POC 3a, 15 | conversation starts in Q1 |

## 12. Working-group cadence

The ACN TWG is new and thinly documented; whoever brings a runnable kit and one reproducible result sets the terms. Target: the M1 kit, the per-provider POC 4 table and one exploratory turn-transport number at the earliest WG meeting after M1, rather than a polished M2. Early and reproducible beats complete.
