# TASKS.md — implementation order and paste-ready prompts

Each substrate task is one PR, tests first, per the CLAUDE.md loop. Tasks are grouped by milestone gate (SPEC 095); nothing after a gate starts before the gate closes (CON-22). **Lab work is never gated**: every milestone has an open exploration slot (`L-*`) and lab spikes may run at any time, in parallel with anything below. Spec numbers refer to `specs/`; the specs marked *(to write)* are drafted as `spec-change` PRs in the task that needs them, before implementation begins.

## M0 — bootstrap and first result (POC 4)

```
T01 bootstrap        → repo skeleton per CON-6; Cargo workspace; rust-toolchain.toml;
                       clippy config (disallowed_methods for CON-5, unwrap/expect/panic
                       denied per CON-19); deny.toml; tools/ci.sh (CON-9);
                       xtask: trace-check (CON-12), docs-inventory, env-hash (CON-7).
                       Self-hosting tests: trace-check must fail on an uncited MUST
                       fixture and on a citation of a non-existent ID.
T02 trace schema     → SPEC 010 (TRC, drafted): OTel span profile + acn.* inventory
                       (TRC-10..21), OTLP-shaped Parquet writer with promoted columns
                       (TRC-25), seeded IdGenerator (TRC-27), bundle + manifest +
                       `acn bundle verify` (TRC-22..24), derived views (TRC-30..35),
                       OTLP export/import (TRC-28). Golden fixtures committed.
                       Add opentelemetry, opentelemetry_sdk, tracing-opentelemetry
                       to the workspace deps.
T03 mock inference   → SPEC 030 (MLM): deterministic OpenAI-compatible HTTP server:
                       pluggable cache-accounting models (explicit breakpoints /
                       automatic prefix / block-granular), prefill time = f(new tokens),
                       decode ITL cadence, streaming SSE; all timing from injected
                       Clock; seeded jitter. In-process for tests. Bundles labelled
                       backend="mockllm" (CON-26): they gate, they do not answer.
T04 harness + knobs  → SPEC 040 (HAR): minimal agent loop (system prompt, tools,
                       tool results, compaction) with switchable discipline knobs:
                       timestamp-in-system-prompt, tool-order stability, tail-restate
                       vs mid-prefix backfill, fork-from-prefix vs per-child prompts,
                       compaction trigger (window-full vs read-cost threshold),
                       cache-breakpoint placement (for explicit-breakpoint providers).
                       Emits TRC call records incl. cached_tokens. Backends: mockllm
                       (default) and ≥2 real providers behind feature `real-api`
                       (Anthropic explicit cache_control, OpenAI automatic prefix;
                       vLLM/SGLang when a node exists).
T05 hypotheses       → SPEC 080 (HYP): hypotheses/<poc>.toml format; verdict predicates
                       over bundle statistics; `acn hyp verdict`; freeze check
                       (env-hash covers hypotheses/). First file: hypotheses/p4.toml.
T05b loop runner     → SPEC 085 (LOOP): `acn loop run` (L1: grid/bisect/random within
                       declared ranges, verdict trajectory, loop report + lab-note draft),
                       `acn evidence verify` (hash chain), direction/gating tests
                       (LOOP-3, LOOP-4). `loop twin` and `loop promote` land with
                       T11b and T30 respectively. First use: POC 4 knob grid.
T06 POC 4 spec+suite → SPEC 100 (P4) *(to write)*: protocol, knob grid, replicate
                       count, control (default config), falsifier (effect < noise floor),
                       expected outcome, **per provider**. tests/accept/p4.rs on mockllm
                       (mock-gated). Then `live` runs against ≥2 real providers with
                       cached-token counters; one bundle and one verdict per provider
                       attached to the PR.
T08 first trace      → lab/trace-capture graduates minimally: capture format + provenance
                       per CON-21; one phone-tethered 5G walk trace lands in
                       scenarios/measured/ (Class C from M0). Do not wait for M4.
T07 M0 gate          → SPEC 095 §M0 acceptance; docs/gates/M0.md; human sign-off.
L-M0 exploration     → starts day one, in parallel with T01:
                       (a) lab/turn-transport — quinn prototype: one QUIC stream per
                           turn, declared deadline, resume-after-gap without re-prefill,
                           prefix-as-delta. Question: how much of POC 11/12/14 does a
                           turn-native transport collapse into one mechanism?
                       (b) lab/trace-capture — Rust client/server logging RTT, loss,
                           connectivity gaps from a tethered phone on a walk.
                       (c) lab/hypotheses/p17-a2a.toml — candidate: agent-to-agent
                           traffic (two harnesses over MCP/A2A across the link) is the
                           traffic class that breaks "agent traffic is ordinary" first.
                       (d) a 200-line discrete-event toy of a turn over a lossy link,
                           to learn which impairment shapes matter before EMU is specified.
                       Lab note per spike, nothing else.
```

## M1 — substrate MVP (POC 16 minimum)

```
T10 link models      → SPEC 020 (EMU) part 1: LinkModel trait; delay/jitter, rate +
                       token bucket, i.i.d. and Gilbert–Elliott burst loss, reorder,
                       scheduled outages (handover gaps); scenario TOML loader with
                       deny_unknown_fields; property tests on model statistics.
T11 sim engine       → EMU part 2: discrete-event engine on SimClock; message-level
                       transport abstraction; bit-identical replay test (CON-5c).
T11b sim↔live twin   → CON-25 + LOOP-12: `acn loop twin` runs the decision-relevant
                       configs of a loop report in `live`, records per-quantity
                       divergence; tolerance from the hypothesis file; refuses
                       non-reproducible L1 input (LOOP-4). Standing test from here on.
T12 live proxy       → EMU part 3: tokio TCP proxy applying the same LinkModel to
                       real sockets; schedule precomputed from seed (CON-5d);
                       boundary timestamps emitted as TRC link-event records.
T13 workload gen     → SPEC 050 (GEN): session/turn/call generator from Appendix C
                       parameter sheet; fan-out; think-time; drives mockllm through
                       the proxy; replay from seed; plain-RPC control workload
                       (multi-step REST) as a first-class generator mode (CON-18).
T14 control plane    → SPEC 070 (CTL): axum HTTP/JSON API: create scenario, start run,
                       poll status, fetch bundle; run registry on disk; endpoints for
                       external inference nodes and partner endpoints; OpenAPI doc
                       generated by docs-inventory.
T15 attribution core → SPEC 090 (ATR): tail decomposition by hop and cause;
                       network-attributable share with control subtraction; CI via
                       bootstrap; heatmap + SVG via plotters; fixed summation order.
T16 kit + regen      → `acn run --from-run-id` regenerates a bundle; second-machine
                       reproduction test; README quickstart; docs/evidence/ pages
                       generated by docs-inventory (LOOP-30). hypotheses/p16.toml.
T17 M1 gate          → SPEC 095 §M1; docs/gates/M1.md. WG package: kit + per-provider
                       POC 4 table + one exploratory turn-transport number.
L-M1 exploration     → graduation review of turn-transport and p17-a2a lab notes; if
                       turn-transport graduates, it becomes SPEC 125 and POC 12/14 are
                       re-cut as its evaluation rather than separate experiments.
                       Open slot for anything from the WG.
```

## M2 — first network results

```
T20 POC 1a           → SPEC 110 (P1A) *(to write)*: 3×3 placement × profile grid;
                       bisection driver for the 5% boundary; heatmap; control = REST
                       workflow on the same grid. hypotheses/p1a.toml. tests/accept/p1a.rs.
T21 POC 1b           → SPEC 111 (P1B): tail anatomy; independent burstiness/jitter
                       sweeps; dominant-term fit. hypotheses/p1b.toml.
T22 POC 11 column    → SPEC 120 (P11): prefix-redundancy measurement on 1a traces;
                       delta-proxy mode in acn-emu (content-aware, off by default);
                       control = REST. hypotheses/p11.toml.
T23 POC 13 column    → SPEC 122 (P13): fan-out burst shape; control = N independent
                       users at uniform random start. hypotheses/p13.toml.
T24 M2 gate          → verdicts for 1a, 1b, 11, 13 with controls; docs/gates/M2.md.
L-M2 exploration     → open slot, plus graduation review of every lab note so far.
```

## M3 — calibration against real inference nodes *(hardware-gated: needs one GPU node running vLLM/SGLang reachable by the control plane; confirm before scheduling)*

```
T30 node adapters    → CTL: adapters for vLLM / SGLang endpoints (metrics scrape,
                       forced re-homing via router config) behind `real-api`;
                       `acn loop promote` (L3) lands here (LOOP-12).
T31 POC 7            → SPEC 130 (P7): prefix-affinity cost curve; regeneration-rate
                       measurement; control = never re-homed. hypotheses/p7.toml.
T32 M3 gate          → docs/gates/M3.md with measured regeneration rate.
```

## M4 — Linux backend and measured traces

```
T40 netem backend    → EMU part 4 (feature `netem`): scenario → tc/netem in netns;
                       ignored-tier tests; live-vs-netem agreement test on synthetic
                       scenarios.
T41 trace library    → extend T08 to three sets: 5G handover walk (from M0), congested
                       Wi-Fi cell, cross-region WAN; anonymisation review; Class C.
T42 POC 12           → SPEC 121 (P12): stream survivability; transports TCP+SSE,
                       HTTP/2, QUIC (quinn), MASQUE-style resume; control = file
                       download. hypotheses/p12.toml.
T43 POC 14           → SPEC 123 (P14): protocol chattiness; control = bare call.
T44 M4 gate          → docs/gates/M4.md; kit release tag.
```

## Later (each gated by its own spec)

```
T50 replayer         → SPEC 060 (RPL): recorded sensor streams → planner stub; scoring;
                       toy closed loop (2-D nav/pick) for 3a and 10.
T51 POC 3a           → with partner endpoint through CTL; hypotheses/p3a.toml.
T52 POC 2a           → router hint ablation. Deferred: needs multiple real inference
                       workers behind a router; on a mock it is circular (CON-26).
T53 POC 5, 6, 8, 10, 15 → per catalogue; each its own spec + hypothesis file.
T54 POC 9            → record the auto-research loop's own sessions as a workload.
```

---

## Paste-ready prompts

**Lab spike:**
```
Read CLAUDE.md §Lab track. Create lab/<slug>/ as a Rust crate. Question: <one sentence>.
Try it the fastest way that works; depend on anything you need. Do not write a spec,
IDs, or a control. Finish with docs/lab/<date>-<slug>.md: question, what was tried,
what was learned (exploratory numbers welcome), and a recommendation:
graduate / park / drop. If graduate, list the MUSTs you think the spec needs.
```


**Kickoff (T01):**
```
Read CLAUDE.md and specs/000-constitution.md. Implement task T01 from TASKS.md:
bootstrap the workspace exactly per CON-6, CON-9, CON-12, CON-19, tests first
(crates/xtask/tests/trace_check_selfhost.rs, crates/xtask/tests/env_hash.rs).
Do not add acn-emu, tokio proxies or any network code yet. Finish with all gates
green and a PR description listing requirement IDs.
```

**Per-task template:**
```
Read CLAUDE.md, specs/000-constitution.md, specs/<NNN>-*.md, and TASKS.md task T<nn>.
List the MUST IDs you will satisfy. Write the tests named in the spec first, each with
a `/// Cites:` doc comment. Implement. Run tools/ci.sh. Open a PR citing IDs.
Ambiguities: ADR per CON-15, do not stall. Do not touch specs/ or hypotheses/.
```

**POC task template (T06, T20–T23, T31, T42–T43, T5x):**
```
Read CLAUDE.md, specs/000-constitution.md, specs/080-hypotheses.md, specs/<NNN>-p<poc>.md
and hypotheses/p<poc>.toml. Confirm the hypothesis file is frozen (cargo xtask env-hash
--check). Write tests/accept/p<poc>.rs first: it MUST run the control named in the
hypothesis file under the same scenario (CON-18) and cite the spec IDs. Implement any
missing substrate feature in its own crate with its own tests. Run the suite in sim
mode, then live mode if the spec asks. Attach the bundle run_id and the output of
`acn hyp verdict` to the PR. If the verdict is "fail", report it; do not edit the
hypothesis.
```

**Spec-drafting template (specs marked *to write*):**
```
Read specs/000-constitution.md and the ACN report v1.2 Appendix E entry for POC <poc>.
Draft specs/<NNN>-p<poc>.md as a `spec-change` PR: status header, purpose, definitions,
numbered MUSTs (prefix P<poc>-n) covering protocol, variables and ranges, measured
quantities, control, replicate count, falsifier predicate, expected outcome, and the
acceptance test names. Draft hypotheses/p<poc>.toml alongside it. Do not implement.
```

**Cross-review (separate session; CON-16 says when it should be requested; it informs, and never replaces, the approving review that a second maintainer makes mandatory):**
```
Review PR <n> against specs/<NNN>. Check: every MUST in scope cited by a test;
CON-5 determinism (no clock/rng leaks; sim replay bit-identical); CON-8 CLI contract;
CON-18 control present in any acceptance suite; edits to specs/ only in a spec-change PR
with rationale and IDs (CON-14); edits to hypotheses/ or the frozen set only in a Class C
PR, which also gets the adversarial prompt below (CON-7, CON-17, HYP-26); error messages
actionable. Comment findings; do not push.
```

**Adversarial review (Class C):**
```
This PR touches the frozen set. Your job is to break the integrity story: find any way
a run's verdict could be changed without changing run_id; any path by which an
automated loop could alter a hypothesis or a measured trace; any statistic in
acn-attrib whose result depends on evaluation order or platform. Report each as a
blocking comment with a reproduction.
```
