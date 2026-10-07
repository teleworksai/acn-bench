# ADR-38 — T13: the workload generator as a sheet of distributions on the harness's run path

**Status:** accepted (T13.1; `spec-change`, SPEC 050 Draft v0.1). **IDs affected:** GEN-1 to GEN-30; HAR-1, HAR-4, HAR-5, HAR-10, HAR-15, HAR-24 to HAR-26, HAR-41 to HAR-43, HAR-50; MLM-11, MLM-40, MLM-50, MLM-51; CON-5, CON-18, CON-26, CON-29, CON-30. **Decided with the maintainer:** the placeholder values and the deferral of the plain-RPC control (2026-10-06).

## Context
T13 asks for a session/turn/call generator "from the Appendix C parameter sheet", with fan-out and think time. It should drive the mock through the proxy, replay from a seed, and offer a plain-RPC control. SPEC 050 did not exist. The repository holds the report's Appendix C *keys* (`docs/report/coverage.toml`) but not its numbers. SPEC 010 defines no span for request/response traffic without a model.

## Decision
- **Placeholder values (the maintainer's choice).** SPEC 050 fixes the sheet's format. `workloads/gen/appendix-c.toml` ships with `placeholder = true`: values of a plausible shape, as the mock profiles are placeholders (MLM-50). The report's numbers replace them in a data-only PR.
- **The plain-RPC control is a later task, T13b (the maintainer's choice).** It needs an `acn.rpc` span in SPEC 010, which is a Class C change. T13 uses only the span kinds that exist.
- **Integer distributions only (GEN-2).** The four forms are a constant, a uniform range, a quantile table and a weighted choice. A heavy tail is written as a quantile table, which is also how a published parameter sheet usually states it (p50, p90, p99). Sampling a lognormal would need `ln` and `exp`, whose results are not bit-identical across platforms, and CON-5 requires bit-identical `sim` bundles on every CI platform.
- **Sub-streams by what they decide (GEN-3), and plans drawn whole (GEN-4).**
  - Adding a session, a turn or a parameter never shifts another's draws. This follows CON-30(b)'s rule for components.
  - A turn's whole plan, its sub-agents' included, is drawn when the turn starts, in a fixed order.
  - Text comes from a stream per lineage.
  - Concurrent sub-agents therefore never race for draws. The first draft let them share their parent's streams "in spawn order"; in `live`, that order is the order responses arrive, which would break the twin (the review caught it).
- **The generator runs on the harness's run path (GEN-20).** `acn-gen` depends on `acn-harness`, which departs from PLAN.md's dependency sketch (`acn-emu` ← `acn-gen`, beside the harness).
  - A second copy of the run path would duplicate a lot: the sim and live environments, the network and the proxy, retries, isolation, the served mock, span recording and bundles. It would drift from the first, and the harness's tests would not cover it.
  - The harness gains a driver seam: what decides each call is either the agent loop (a workload) or the generator (a sheet).
  - The seam is a trait the harness defines and `acn-gen` implements, and `acn-cli` wires them together. The harness never depends on `acn-gen`, so there is no cycle.
  - The seam also takes the workload's identity and the producer's resource name. The `acn.harness.*` attributes that SPEC 010 lists for the harness alone are omitted from generator spans (GEN-21).
  - PLAN.md's dependency line is updated to say so.
- **How the generator steers the mock (GEN-11).** The mock decides each reply from the request (MLM-40), so the generator steers it by what it sends.
  - Every call carries the same tools, one `gen_<class>` per drawable class, so prompts extend one another and the cache behaves as it does for an agent. Only `tool_choice` differs: `allowed_tools` naming the drawn class on a tool call, and `none` on the answer and on a summary call.
  - The first draft sent the tools only on tool calls, which broke prompt extension. It also left the drawn class unused, because the mock picks tools by position. The review caught both.
  - The profile's `tool_calls_per_turn` must cover the longest chain. The shipped profiles allow one, so T13 adds `mock-agentic` with a long limit and a wide output range, which MLM-51 permits as a new name for new constants.
- **Output length is a cap.** Only `answer_tokens` is drawn, and it is sent as `max_tokens`. The mock draws the answer's length from its profile and caps it there (MLM-40). A tool-call reply has a fixed size whatever the cap, so the first draft's `tool_call_tokens` is dropped.
- **The harness's rules wherever they apply.**
  - Fan-out is HAR-5's: the first tool call spawns, at depth 1, and sub-agents never spawn.
  - Compaction is HAR-4's with HAR-15's `window_full` trigger, with the sheet's summary instruction and output cap.
  - A turn's outcome is HAR-1's and HAR-24's: `success`, or `aborted` after exhausted retries, with the session going on.
  - The first draft had a depth parameter, its own compaction and a `failure` outcome, each contradicting the harness. Each is gone.
- **Sessions are concurrent (GEN-10).** This replaces HAR-1's one session per task and HAR-43's order within a replicate. Replicates keep `run.order`, and `sim` batching stays HAR-41's.
- **Knobs stay at their defaults in v0.1 (GEN-21).** A knob `vary` is refused. What `fanout_prompting` or the tool-order knobs mean for synthetic sessions is SPEC 050 §6's question 3.

## Consequences
- T13 is planned as:
  - T13.1, this `spec-change`;
  - T13.2, `acn-gen`: the sheet, its draws and their known-answer vectors;
  - T13.3, the harness's driver seam, the generator's sessions on it, `acn gen run`, the `mock-agentic` profile and the shipped sheet.
  
  All three are Class B: nothing in the frozen set changes.
- T13b (the plain-RPC control) follows, after a SPEC 010 addition that the maintainer merges as an `env-change`.
- POCs that need fleet-shaped traffic (POC 1a, 1b, 13) can run generator bundles beside harness bundles: both carry the same spans and views. Like every mock bundle, a generator bundle is never cited (CON-26).

## T13.2 notes
- **The ranged-integer sampler** is the harness's `acn_harness::agent::below`: exact rejection on 64 bits, the one the harness already draws with. CON-5(a) pins the output of a sampler by golden vector rather than naming an algorithm; `crates/acn-gen/tests/sheet.rs` pins the generator's first draws.
- **Validation.** A uniform range over all 64 bits is refused, so `max − min + 1` never overflows. `tool_class` lists class names, and the sheet holds a choice over their positions in TRC-13's order.
- **`mock-agentic`** is `mock-auto`'s constants with `tool_calls_per_turn = 64` and answers of 16 to 4096 tokens: placeholders, as every profile is. It changes the embedded profiles' BLAKE3, which only `GET /v1/models` reports (MLM-50), so no existing bundle changes. The mock's test of the model list now counts the embedded profiles rather than three.
- **After review (PR 50).**
  - **Size limits.** A sheet is refused at load when a count (sessions, turns, sub-agents) can exceed 1 000 000, or a text (any `*_tokens`) can exceed 2^24 tokens. A plan and a text are held in memory, and a value near `i64::MAX` would otherwise abort the run.
  - **The range of a sheet's values.** TOML integers are signed, so a sheet can state only values from 0 to `i64::MAX`. The guard against a uniform range over all 64 bits is defensive.
  - **Plans fail closed.** A class or parameter missing from a loaded sheet is an internal error, never a default value.
