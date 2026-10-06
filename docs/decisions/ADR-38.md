# ADR-38 — T13: the workload generator as a sheet of distributions on the harness's run path

**Status:** accepted (T13.1; `spec-change`, SPEC 050 Draft v0.1). **IDs affected:** GEN-1 to GEN-30; HAR-4, HAR-5, HAR-24 to HAR-26, HAR-42; MLM-11, MLM-40, MLM-50, MLM-51; CON-5, CON-18, CON-26, CON-29, CON-30. **Decided with the maintainer:** the placeholder values and the deferral of the plain-RPC control (2026-10-06).

## Context
T13 asks for a session/turn/call generator "from the Appendix C parameter sheet", with fan-out and think time. It should drive the mock through the proxy, replay from a seed, and offer a plain-RPC control. SPEC 050 did not exist. The repository holds the report's Appendix C *keys* (`docs/report/coverage.toml`) but not its numbers. SPEC 010 defines no span for request/response traffic without a model.

## Decision
- **Placeholder values (the maintainer's choice).** SPEC 050 fixes the sheet's format. `workloads/gen/appendix-c.toml` ships with `placeholder = true`: values of a plausible shape, as the mock profiles are placeholders (MLM-50). The report's numbers replace them in a data-only PR.
- **The plain-RPC control is a later task, T13b (the maintainer's choice).** It needs an `acn.rpc` span in SPEC 010, which is a Class C change. T13 uses only the span kinds that exist.
- **Integer distributions only (GEN-2).** The four forms are a constant, a uniform range, a quantile table and a weighted choice. A heavy tail is written as a quantile table, which is also how a published parameter sheet usually states it (p50, p90, p99). Sampling a lognormal would need `ln` and `exp`, whose results are not bit-identical across platforms, and CON-5 requires bit-identical `sim` bundles on every CI platform.
- **Sub-streams by what they decide (GEN-3).** Adding a session, a turn or a parameter never shifts another's draws. This follows CON-30(b)'s rule for components.
- **The generator runs on the harness's run path (GEN-20).** `acn-gen` depends on `acn-harness`, which departs from PLAN.md's dependency sketch (`acn-emu` ← `acn-gen`, beside the harness).
  - A second copy of the run path would duplicate a lot: the sim and live environments, the network and the proxy, retries, isolation, the served mock, span recording and bundles. It would drift from the first, and the harness's tests would not cover it.
  - The harness gains a driver seam: what decides each call is either the agent loop (a workload) or the generator (a sheet).
- **How the generator controls chain length.** The mock decides tool calls from the request (MLM-40), so the generator steers it by what it sends:
  - every tool call carries the tools;
  - the answer carries `tool_choice: "none"`;
  - the profile's `tool_calls_per_turn` must cover the longest chain, or the sheet is refused.
  
  The shipped profiles allow one tool call per turn. T13 therefore adds a profile, `mock-agentic`, with a long limit, which MLM-51 permits as a new name for new constants.
- **Output lengths are caps.** `answer_tokens` and `tool_call_tokens` are sent as the request's token limit. The mock draws an answer's length from its profile and caps it there (MLM-40). The sheet therefore bounds what the report calls output length, and the recorded `output_tokens` is what the mock returned.
- **Turn outcome (GEN-15).** The generator has no task to check. Its rule is mechanical: success when every call got a response, timeout when deadlines were exhausted, failure otherwise. It is part of the sheet's semantics, never the mock's (TRC-11).

## Consequences
- T13 is planned as:
  - T13.1, this `spec-change`;
  - T13.2, `acn-gen`: the sheet, its draws and their known-answer vectors;
  - T13.3, the harness's driver seam, the generator's sessions on it, `acn gen run`, the `mock-agentic` profile and the shipped sheet.
  
  All three are Class B: nothing in the frozen set changes.
- T13b (the plain-RPC control) follows, after a SPEC 010 addition that the maintainer merges as an `env-change`.
- POCs that need fleet-shaped traffic (POC 1a, 1b, 13) can run generator bundles beside harness bundles: both carry the same spans and views.
