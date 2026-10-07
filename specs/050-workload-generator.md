# SPEC 050 — The workload generator

**Status:** Draft v0.2 (October 2026; v0.2: GEN-11's tool descriptions carry HAR-42's marker, and GEN-13 says how HAR-11's timestamp bounds the shared system prefix, issue #53; v0.1 written for T13). **Inherits:** SPEC 000, 010, 020, 030, 040. **Prefix:** GEN. **Crates:** `acn-gen`, `acn-harness` (the driver seam), `acn-cli`.
**Purpose:**
- define the traffic of many agent sessions as data: a *sheet* of the parameters the report's traffic model names (its §3.4 and Appendix C), each a distribution;
- define how a seeded generator draws sessions, turns and calls from a sheet, and sends them to the mock in `sim` or `live`, over a scenario's network when one is given;
- keep the generator on the harness's run path (SPEC 040), so that its bundles, spans, retries and network are the harness's, and only what decides each call differs.

## 0. Terms

- **Sheet** — a TOML file of generator parameters (GEN-1). Its BLAKE3 is the run's `workload_hash`.
- **Draw** — one value of a distribution, taken from a named random sub-stream (CON-30(b)).
- **Plan** — every draw of one turn, made when the turn starts (GEN-4).
- **Chain** — the calls of one turn on one lineage: zero or more tool calls, then one answer call. A **sub-chain** is the chain of a sub-agent spawned by fan-out (GEN-12).
- The plain-RPC control (CON-18) is not in this version. It needs a span for non-model traffic that SPEC 010 does not define, so it is a later task, T13b (§6).

## 1. The sheet

**GEN-1** A sheet MUST be a TOML file, parsed with unknown keys rejected at every level and every parameter stated, nothing implied at load time. It carries:
- `schema_version = 1`, `placeholder` (bool) and `doc` (string);
- `model`, an embedded mock profile (MLM-50);
- `sessions`, the number of sessions per replicate (an integer, at least 1);
- `system_tokens`, `summary_instruction_tokens` and `summary_max_tokens` (integers, the last two as HAR-4's summary instruction and output cap), and `compact_at_tokens` (an integer; 0 turns compaction off);
- one distribution (GEN-2) for each of these:
  - **session level:** `session_start_ns` (each session's start after the replicate's), `turns_per_session` (at least 1), `think_time_ns`;
  - **turn level:** `chain_length` (the tool calls before the answer), `fanout_width` (sub-agents spawned by a turn, 0 for none);
  - **call level:** `user_tokens` (a turn's new user text), `answer_tokens` (the answer call's output cap), and per tool class `tool_result_tokens` and `tool_duration_ns`;
  - **tools:** `tool_class`, a weighted choice (GEN-2) of SPEC 010's classes other than `subagent` (TRC-13).

A sheet MUST be refused at load, naming the parameter, when:
- `schema_version` is not 1, or `model` is not an embedded profile;
- a per-class parameter is missing for a class `tool_class` can draw;
- `fanout_width` can be above 0 while `chain_length` can be 0, since the spawn is a tool call (GEN-12);
- the profile's `tool_calls_per_turn` (MLM-40) is below the largest chain length the sheet can draw.

The repository ships `workloads/gen/appendix-c.toml` with `placeholder = true`. Its values have a plausible shape but are not the report's, until the report's Appendix C numbers replace them in a data-only PR.

**GEN-2** A distribution MUST be one of the following, every value an unsigned 64-bit integer:
- `{ const = n }`;
- `{ uniform = [min, max] }` with `min ≤ max`: drawn uniformly from `min..=max` by the ranged-integer sampler of CON-5(a);
- `{ quantiles = [[p, v], …] }`: a piecewise-linear inverse CDF.
  - The `p` are in parts per million, strictly increasing, the first 0 and the last 1 000 000; the `v` do not decrease.
  - A draw takes `u` uniformly from `0..=1_000_000`, finds the first segment with `p_i ≤ u ≤ p_{i+1}`, and returns `v_i + ⌊(v_{i+1} − v_i)(u − p_i) / (p_{i+1} − p_i)⌋`, computed in 128-bit integers.
- `{ weighted = [[value, weight], …] }`, permitted for `tool_class` and for any integer parameter:
  - weights are positive integers whose sum fits in 64 bits, and values are distinct;
  - a draw takes `r` uniformly from `0..total` and returns the first entry, in listed order, whose cumulative weight exceeds `r`.

No draw uses floating point or a transcendental function, so a draw is bit-identical on every platform (CON-5).

## 2. Sessions, turns and calls

**GEN-3** Draws MUST come from per-replicate sub-streams named by what they decide (CON-30(b)), so that a change to one session, turn or parameter shifts no other draw:
- `gen.session.<k>` for session *k*'s start and turn count, and each turn's think time, in turn order;
- `gen.plan.<k>.<t>` for the plan of turn *t* of session *k* (GEN-4);
- `gen.text.<k>.<t>.<c>` for the bytes of the text of lineage *c* of that turn (*c* = 0 for the main lineage, 1 + *i* for sub-agent *i*);
- `gen.text.system` for the system message every session shares (GEN-13).

**GEN-4** A turn's plan MUST be drawn whole from `gen.plan.<k>.<t>` when the turn starts, before its first call, in this order:
1. the main lineage's `user_tokens`, `chain_length` and `fanout_width`;
2. for each of its tool calls in order: its class (`subagent` for the first when the width is above 0, otherwise `tool_class`), then that class's `tool_duration_ns` and `tool_result_tokens` (none for `subagent`);
3. its `answer_tokens`;
4. for each sub-agent *i* in index order, the same as steps 1 to 3 for its sub-chain, without a width.

A plan therefore never depends on when a response arrives, and a `sim` run and its `live` twin draw the same plans.

**GEN-10** A replicate MUST run its `sessions` sessions concurrently, session *k* starting at its drawn `session_start_ns` after the replicate's start. This replaces HAR-1's one session per task in order and HAR-43's workload order within a replicate; replicates still run in the order HAR-43 draws.
- Each session runs `turns_per_session` turns in order.
- Before every turn but the first, the session waits its drawn `think_time_ns`, measured from the end of the previous turn.
- A session's `acn.session` span starts at its drawn start.
- In `sim`, calls that reach the mock at one instant form one batch (HAR-41), whichever session they come from.

**GEN-11** Every call of a session MUST carry the same `tools` and differ only in `tool_choice`. So each call's prompt extends the last (MLM-10), and caching behaves as it does for an agent.
- **The tools.** There is one tool per class `tool_class` can draw, plus `subagent` when `fanout_width` can be above 0. Each is named `gen_<class>`, with the description `<marker> A <class> tool.`, where `<marker>` is the replicate's isolation marker that HAR-42 puts at the start of every tool definition's description, parameters `{"type": "object", "properties": {}}`, listed in TRC-13's class order.
- **A turn.** It appends a `user` message of `user_tokens` tokens of text (GEN-13), then makes its tool calls, then its answer call.
- **A tool call.** It sends `tool_choice: {"type": "allowed_tools", "allowed_tools": {"tools": [{"type": "function", "function": {"name": "gen_<class>"}}]}}`, so the mock calls the drawn class (MLM-40). It then waits that class's drawn `tool_duration_ns`, and appends the assistant message and a `tool` message of `tool_result_tokens` tokens.
- **The answer call.** It sends `tool_choice: "none"` and `max_tokens = answer_tokens`. The mock draws the answer's length from its profile and caps it there (MLM-40).

**GEN-12** Fan-out MUST follow HAR-5. When a turn's width *w* is above 0:
- its first tool call is the `subagent` call;
- its result is the *w* sub-agents' answers, concatenated in index order;
- sub-agents do not spawn;
- each sub-agent runs its sub-chain with its own system message (the shared one) and one `user` message of its drawn `user_tokens`, on the tools without `subagent`;
- the sub-agents run concurrently, and the turn's next call waits for all of them;
- spans and lineages are recorded as HAR-5 and TRC-14 record fan-out.

**GEN-13** Synthetic text MUST be made of words drawn from its lineage's text stream (GEN-3): four lowercase ASCII letters and no character JSON escapes, separated from the next by nothing. A message of *n* tokens has 4*n* content bytes. MLM-11 tokenizes the whole canonical prompt, so its token count is close to, not exactly, *n*. The system message of every session is the same `system_tokens` words, drawn from `gen.text.system`. The run's isolation marker (HAR-42) prefixes it as it prefixes the harness's, and, with HAR-11's `timestamp_in_system_prompt` on (its default, which GEN-21 keeps), the turn's timestamp line follows the marker. Sessions and turns therefore share a cached prefix up to that line, and the system words after it are shared only by the calls of one turn. Which knobs apply to synthetic traffic, and how, is §6's question 3.

**GEN-14** Compaction MUST be HAR-4's on the main lineage, with the trigger `window_full` of HAR-15 and the sheet's `compact_at_tokens` as the threshold (0: never):
- the summary call carries the tools and `tool_choice: "none"`;
- its instruction is `summary_instruction_tokens` words from the lineage's text stream;
- its output is capped at `summary_max_tokens`;
- the messages it keeps and replaces are HAR-4's.

**GEN-15** A turn's outcome MUST follow HAR-1 and HAR-24, never the mock's reply (TRC-11):
- `success` when every call of the turn, its sub-chains' included, got a response;
- `aborted` when a call failed after its retries (HAR-24), which ends the turn there; the session goes on to its next turn.

The sheet has no deadline, so a generator turn is never `timeout`.

## 3. Running

**GEN-20** The generator MUST run on the harness's run path (SPEC 040):
- its environments in `sim` and `live`;
- its retries and timeouts (HAR-24) and its isolation (HAR-42);
- its network in `sim` with a scenario (SPEC 020 §4), its per-replicate proxy in `live` (SPEC 020 §5), and its served mock (HAR-26);
- its spans and bundle (TRC-10 to TRC-15, TRC-22).

Only what decides each call is the generator's. It is plugged into the harness through a driver seam that the harness defines and `acn-gen` implements; the harness never depends on `acn-gen`.

**GEN-21** A generator run's identity MUST follow CON-29 as a harness run's does, with the sheet as the workload (`workload_hash` is its BLAKE3, CON-27(a)), `backend = mockllm` and the sheet's `model`. The other inputs are HAR-50's: the hypothesis or `none` with its seed, the arm, the replicate count, the `vary.` parameters and the `opt.` options.
- In this version the knobs of HAR-10 are at their defaults, so a `vary` of a knob to another value is refused. What a knob means for synthetic traffic is §6's question 3.
- The session spans carry `acn.role` as the arm and `acn.workload.hash` as the sheet's hash.
- Their resource names `acn-gen` (TRC-19), and the `acn.harness.*` attributes that SPEC 010 lists for the harness alone are omitted.

**GEN-22** `acn gen run --sheet <file>` MUST take HAR-50's other flags (`--mode`, `--arm`, `--replicates`, `--vary`, the `opt.` options, `--hypothesis` or `--seed`, `--runs-dir`) and `--scenario <file>`. It MUST print one JSON object (CON-8) with `ok`, `run_id`, `bundle_digest`, `sessions` and `calls`. In `live` with no `--endpoint`, the mock is served by the harness (HAR-26).

**GEN-23** In `sim`, the same sheet, seed and scenario MUST give a bit-identical bundle (CON-5).

## 4. What the generator records

**GEN-30** The generator MUST record nothing beyond SPEC 010.
- Sessions, turns, calls, tools and sub-agents are the harness's spans, and the Appendix C keys of `docs/report/coverage.toml` resolve from them as they do for harness bundles.
- The views count what was sent:
  - a turn's `chain_length` is its main-lineage calls on the critical path: the drawn tool calls, the answer and any summary call;
  - `acn.call.index` counts those calls from 0.
- Generator bundles run on the mock, so they are never cited (CON-26). A placeholder sheet is a second reason: its shape is not yet the report's.

## 5. Acceptance tests

- `crates/acn-gen/tests/sheet.rs` — GEN-1, GEN-2:
  - the shipped sheet loads;
  - each refusal of GEN-1 and GEN-2 fires: an unknown key, a missing parameter, `schema_version`, an unknown profile, `min > max`, quantiles not from 0 to 1 000 000 or out of order, a duplicate or zero weight, a missing per-class entry, fan-out with a possible zero chain, a chain longer than the profile allows;
  - draws match known-answer vectors.
- `crates/acn-gen/tests/plan.rs` — GEN-3, GEN-4:
  - changing one parameter, session or turn changes no other draw;
  - plans are drawn whole and in GEN-4's order.
- `crates/acn-gen/tests/sessions.rs` — GEN-10 to GEN-15:
  - session starts, think times, chain lengths (as GEN-30 counts them), tool classes, fan-out and compaction appear in the views as planned;
  - every call carries the same tools, and each prompt extends the last;
  - compaction is HAR-4's;
  - a failed call aborts its turn and the session goes on.
- `tests/accept/gen_sim.rs` — GEN-20 to GEN-23:
  - a run twice in `sim` is bit-identical;
  - a scenario's network applies;
  - the CLI prints one object and refuses a knob `vary`;
  - a `live` run on the served mock draws the same plans as its `sim` twin (its tool classes, chain lengths and widths), while the mock's outputs and the timing are the mode's own.

## 6. Open questions

1. **The plain-RPC control (CON-18, T13b).** A generator mode that sends the same sessions as plain request/response calls with no model in the loop, to a responder served as HAR-26 serves the mock. It needs an `acn.rpc` span in SPEC 010, a Class C change.
2. **Fleet-level parameters.** The diurnal and weekly pattern and cross-session prefix overlap (Appendix C fleet level) are not drawn here. They need a fleet of runs and prefix-block hashes respectively (SPEC 010 question 1).
3. **Knobs on synthetic traffic.** Which of HAR-10's knobs apply to generated sessions, and how (HAR-11 timestamps and HAR-12 permutations do; HAR-14's fork prompting meets GEN-12's own sub-agent system message).
4. **Calibration.** The placeholder sheet is replaced by the report's numbers, and later by measured ones (L3, SPEC 085), as data-only PRs.
