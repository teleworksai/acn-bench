# SPEC 050 — The workload generator

**Status:** Draft v0.1 (October 2026; written for T13). **Inherits:** SPEC 000, 010, 020, 030, 040. **Prefix:** GEN. **Crates:** `acn-gen`, `acn-cli`.
**Purpose:**
- define the traffic of many agent sessions as data: a *sheet* of the parameters the report's traffic model names (its §3.4 and Appendix C), each a distribution;
- define how a seeded generator draws sessions, turns and calls from a sheet, and sends them to the mock in `sim` or `live`, over a scenario's network when one is given;
- keep the generator on the harness's run path (SPEC 040), so that its bundles, spans, retries and network are the harness's, and only what decides each call differs.

## 0. Terms

- **Sheet** — a TOML file of generator parameters (GEN-1). Its BLAKE3 is the run's `workload_hash`.
- **Draw** — one value of a distribution, taken from a named random sub-stream (CON-30(b)).
- **Chain** — the calls of one turn on one lineage: zero or more tool calls, then one answer. A **sub-chain** is the chain of a sub-agent spawned by fan-out.
- The plain-RPC control (CON-18) is not in this version. It needs a span for non-model traffic that SPEC 010 does not define, so it is a later task: a SPEC 010 addition and a generator mode (§6).

## 1. The sheet

**GEN-1** A sheet MUST be a TOML file, parsed with unknown keys rejected at every level and every parameter stated, nothing implied at load time. It carries:
- `schema_version = 1`, `placeholder` (bool) and `doc` (string);
- `model`, the mock profile it is drawn for (MLM-50), whose `tool_calls_per_turn` MUST be at least the largest chain length the sheet can draw, or the sheet is refused at load naming the profile;
- `sessions`, the number of sessions per replicate (an integer, at least 1);
- one distribution (GEN-2) for each of these:
  - **session level:** `session_start_ns` (each session's start after the replicate's), `turns_per_session`, `think_time_ns`;
  - **turn level:** `chain_length` (the tool calls before the answer), `fanout_width` (sub-agents spawned by a turn, 0 for none), `fanout_depth` (levels of spawning, at least 1 when the width is not 0);
  - **call level:** `user_tokens` (a turn's new user text), `tool_result_tokens` by tool class, `answer_tokens` and `tool_call_tokens` (the output caps of the two call types);
  - **tools:** `tool_class` (a weighted choice of SPEC 010's classes), and `tool_duration_ns` by class.
- `system_tokens` (an integer) and `compact_at_tokens` (an integer, or 0 for never). A context that would exceed `compact_at_tokens` is compacted first, as HAR-4 compacts (GEN-14).

The repository ships `workloads/gen/appendix-c.toml` with `placeholder = true`: values of a plausible shape, not the report's, until the report's Appendix C numbers replace them in a data-only PR. Bundles from a placeholder sheet are exploratory and never cited (CON-26 already makes every mock bundle uncitable; the flag says why the shape is not yet the report's either).

**GEN-2** A distribution MUST be one of:
- `{ const = n }`;
- `{ uniform = [min, max] }`, an integer drawn uniformly from `min..=max` by the ranged-integer sampler of CON-5(a);
- `{ quantiles = [[p, v], …] }`, a piecewise-linear inverse CDF. Each `p` is in parts per million, strictly increasing from 0 to 1 000 000; each `v` is an integer, non-decreasing. A draw takes `u` uniformly from `0..1_000_000` and interpolates between the two points around it, in integer arithmetic, rounding toward the lower value;
- `{ weighted = [[value, weight], …] }`, for `tool_class` and other choices: positive integer weights, a value drawn with probability weight / total by the ranged-integer sampler.

No draw uses floating point or a transcendental function, so a draw is bit-identical on every platform (CON-5).

**GEN-3** Draws MUST come from per-replicate sub-streams named by what they decide (CON-30(b)), so that changing one parameter never shifts another's draws:
- `gen.session.<k>` for session *k*'s start, turn count and think times;
- `gen.turn.<k>.<t>` for turn *t* of session *k*: its chain length, fan-out and text lengths;
- `gen.tool.<k>.<t>` for its tools' classes, durations and result sizes;
- `gen.text.<k>` for the bytes of session *k*'s synthetic text.

A sub-chain draws from its parent turn's streams, in spawn order.

## 2. Sessions, turns and calls

**GEN-10** A replicate MUST run `sessions` sessions concurrently, session *k* starting at its drawn `session_start_ns` after the replicate's start. Each session runs `turns_per_session` turns in order. Before every turn but the first, the session waits its drawn `think_time_ns`, measured from the end of the previous turn.

**GEN-11** A turn MUST:
1. append a `user` message of `user_tokens` tokens;
2. make `chain_length` tool calls;
3. make one answer call.

The calls work as follows:
- **A tool call** carries the sheet's tool definitions and caps its output at `tool_call_tokens`. The mock replies with a tool call (MLM-40), whose execution waits `tool_duration_ns` of a drawn class and appends a `tool` message of `tool_result_tokens` for that class.
- **The answer call** carries `tool_choice: "none"` and caps its output at `answer_tokens`.

Every call sends the whole conversation so far, so each call's prompt extends the last one's (MLM-10), and caching behaves as it does for an agent.

**GEN-12** Fan-out MUST work as follows.
- A turn with a `fanout_width` *w* greater than 0 spawns *w* sub-agents after its first tool call. They run concurrently, each a sub-chain of its own.
- A sub-chain has its own system message and one `user` message of `user_tokens`. Its chain length is drawn as a turn's is.
- A sub-chain spawns again while the drawn depth allows.
- The turn's next call waits for every sub-agent. Their answers return as the result of the spawning tool call, which has class `subagent`.

Spans and lineages follow HAR-5 and TRC-14, as the harness records fan-out.

**GEN-13** Synthetic text MUST be made of 4-byte words drawn from `gen.text.<k>`, so that *n* tokens are exactly *n* words (MLM-11). A message of *n* tokens is therefore 4*n* bytes of content. The system message of every session is the same `system_tokens` words from a fixed stream, `gen.text.system`, so that sessions share a system prefix. The tool definitions are fixed by the sheet's classes.

**GEN-14** Compaction: before a call whose prompt would exceed `compact_at_tokens`, the lineage MUST make one summary call (output capped at `answer_tokens`) and replace its messages after the system message with one `user` message holding that summary. This is the harness's compaction (HAR-4). It is recorded as the harness records it (TRC-11 `acn.turn.compaction`).

**GEN-15** A turn's outcome MUST be decided by the generator's own rule, which is part of the hashed sheet's semantics, never by the mock (TRC-11):
- `success` when every call of the turn, its sub-chains' included, ended with a response;
- `timeout` when a call exhausted its retries on deadlines;
- `failure` otherwise.

## 3. Running

**GEN-20** The generator MUST run on the harness's run path (SPEC 040): its environments in `sim` and `live`, its retries and timeouts (HAR-24), its isolation marker (HAR-42), its network in `sim` with a scenario (SPEC 020 §4) and its per-replicate proxy in `live` (SPEC 020 §5), its served mock (HAR-26), and its spans and bundle (TRC-10 to TRC-15, TRC-22). Only what decides each call is the generator's.

**GEN-21** A generator run's identity MUST follow CON-29 as a harness run's does, with these inputs:
- the sheet as the workload (`workload_hash` is its BLAKE3, CON-27(a));
- `backend = mockllm` and the sheet's `model`;
- the hypothesis or `none`, with its seed;
- the arm and replicate count;
- the `opt.` options of HAR-25.

The session spans carry `acn.role` as the arm, and `acn.workload.hash` as the sheet's hash.

**GEN-22** `acn gen run --sheet <file> [--mode sim|live] [--scenario <file>] [--replicates n] [--seed n | --hypothesis <file>] [--endpoint <url>] [--runs-dir <dir>]` MUST print one JSON object (CON-8): `ok`, `run_id`, `bundle`, `sessions`, `calls`. In `live` without `--endpoint`, the mock is served by the harness (HAR-26).

**GEN-23** In `sim`, the same sheet, seed and scenario MUST give a bit-identical bundle (CON-5). The generator draws nothing from the clock.

## 4. What the generator records

**GEN-30** The generator MUST record nothing beyond SPEC 010:
- sessions, turns, calls, tools and sub-agents are the harness's spans;
- the drawn values are visible through the views (TRC-34), the Appendix C keys of `docs/report/coverage.toml` resolving as they do for harness bundles;
- its spans' resource names `acn-gen` as `service.name` (TRC-19).

## 5. Acceptance tests

- `crates/acn-gen/tests/sheet.rs` — GEN-1, GEN-2:
  - the shipped sheet loads;
  - an unknown key, a missing parameter, quantiles out of order and a chain longer than the profile allows are each refused;
  - draws match known-answer vectors.
- `crates/acn-gen/tests/streams.rs` — GEN-3: changing one parameter changes no other parameter's draws.
- `crates/acn-gen/tests/sessions.rs` — GEN-10 to GEN-15:
  - session starts, think times, chain lengths, fan-out and compaction appear in the views as drawn;
  - each call's prompt extends the last;
  - outcomes follow GEN-15.
- `tests/accept/gen_sim.rs` — GEN-20 to GEN-23:
  - a run twice in `sim` is bit-identical;
  - a scenario's network applies;
  - the CLI prints one object;
  - a `live` run on the served mock records the same draws as its `sim` twin.

## 6. Open questions

1. **The plain-RPC control (CON-18, T13b).** A generator mode that sends the same sessions as plain request/response calls with no model in the loop, to a responder served as HAR-26 serves the mock. It needs an `acn.rpc` span in SPEC 010, a Class C change.
2. **Fleet-level parameters.** The diurnal and weekly pattern and cross-session prefix overlap (Appendix C fleet level) are not drawn here. They need a fleet of runs and prefix-block hashes respectively (SPEC 010 question 1).
3. **Calibration.** The placeholder sheet is replaced by the report's numbers, and later by measured ones (L3, SPEC 085), as data-only PRs.
