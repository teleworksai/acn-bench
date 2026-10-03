# ADR-17 — T04: the harness, and the readings SPEC 040 leaves open

**Status:** accepted (T04; Class B, with a Class C part and a `spec-change` part). **IDs affected:** HAR-1 to HAR-5, HAR-10 to HAR-17, HAR-20 to HAR-25, HAR-30 to HAR-34, HAR-40 to HAR-43, HAR-50 to HAR-52, HAR-60, HAR-61; TRC-10, TRC-11, TRC-12, TRC-20; MLM-4, MLM-21, MLM-60; CON-26, CON-29.

## Context
SPEC 040 states what the harness does to the bytes it sends and how a run is recorded. Implementing it needed choices the spec does not make, one change to the frozen attribute inventory, and one amendment to SPEC 010. It also turned up one property of the mock that the maintainer should see.

## Decision

### The run path
- **Sim is a small discrete-event scheduler.** In `sim`, every lineage's pending wait or call is registered with a scheduler that owns the replicate's `SimClock`. It wakes due waits first. It then hands every call registered at the current instant to the mock as one `Mock::handle_batch`. Only when neither is pending does it move the clock to the next wait. Simultaneous sub-agent calls therefore take MLM-7's order, never the executor's (HAR-41). `live` runs the same agent code on tokio and the wall clock.
- **Tool sets are per task.** A `[[task]]` names the tools the agent has, in presentation order. With the mock's reply policy (MLM-40), the tool at index 0 is the one called, so a workload needs a per-task order to exercise both reading and fan-out. HAR-60 lists task fields without this one; it is an addition, not a change.
- **Tools.**
  - A `subagent` tool has `width` and `child` instead of `result_bytes` and `duration_ns`: its result is the children's answers joined by `\n`, and its duration is theirs.
  - A call to a tool the lineage does not have gets an error result of class `other`, with no duration. A child under `fork_from_prefix` sees the parent's tools, so its call to the subagent tool is refused this way (HAR-5: sub-agents never spawn).
- **Think time.** A turn's `think_time_ns` is the wait before it; the first turn's is ignored.
- **Compaction.**
  - A compaction call does not count towards `max_calls_per_turn`.
  - The messages of the current turn survive it.
  - `read_cost_threshold` reads the previous non-compaction call of the lineage, so a large compaction call cannot trigger another.
  - When the model answers a compaction call with a tool call (the mock always does, since the summary instruction is a fresh user message and tools are present), the summary is empty.
- **`acn.turn.first_useful_result_ms`** is the first token of a successful turn's final answer.

### Calls
- **Retries.**
  - `acn.call.wire_bytes_*` sum every attempt.
  - `acn.call.ttft_ms` is measured from the first attempt's start.
  - A 4xx other than 429 is not retried: it ends with `stop_reason = other` and `error_class = http_<status>`.
  - A response the frozen mapping cannot normalise is an error of the run, not a silent gap.
- **Chat Completions encoding.**
  - The system prompt is always one text part.
  - `rolling_tail` puts its breakpoint on the last message object.
  - Marking therefore changes nothing but the `cache_control` members, which MLM-10 strips. Without this, marking turned a string into a part array, so the marked prefix never matched on the next call, and placement moved cached tokens on the automatic-prefix and block-granular profiles, which ignore breakpoints.
  - The token limit is sent as `max_completion_tokens`, and a streamed request asks for `stream_options.include_usage`.
- **Backend identity (HAR-23, MLM-4, CON-26).** A mismatch stops the run with `backend_mismatch` and leaves no bundle. The harness does not relabel the run as `mockllm`, because `run_id` already carries the configured backend. Every response is inspected only when the configured backend is not the mock.
- **Credentials.**
  - The credentials module is always compiled, because reading a variable reaches no provider. Using credentials for a real backend needs `real-api` (HAR-20).
  - An endpoint URL with user information is refused.
  - `LiveEnv`'s `Debug` output never prints headers.

### Recording
- **No scenario yet.** `scenario_hash` is 32 zero bytes, as `none` is for a hypothesis, until SPEC 020 gives the harness a link.
- **Execution order.**
  - The replicate order is always drawn from `run.order`.
  - It is recorded only in `live`, because the manifest admits `execution_order` only there (TRC-22).
  - Entries are written `<arm>/<i>`.
- **`started_at`** comes from `acn_emu::clock::wall_time_utc`, the one sanctioned read of the wall clock (ADR-8).
- **Hypothesis files, until `acn-hyp` (T05).** The harness reads only:
  - `[poc].id`;
  - the `kind` of each `[varies]` parameter, to type `--vary` values (a `range` is a float, CON-27(c));
  - `[design].seed`, for a candidate.

  The preflight checks status by location and by `env-hash.json`. The seed is HYP-9's derived seed.
- **Workload temperature** is limited to [0, 2]. In that range `serde_json` and CON-27(c) write every float alike.
- **GenAI cache attributes** are written as `gen_ai.usage.cache_read.input_tokens` and `gen_ai.usage.cache_creation.input_tokens`, the v1.40 names. Nothing reads them (TRC-3), so a rename moves no verdict.

### The frozen inventory and SPEC 010
- HAR-25 makes `opt.endpoint`, `opt.max_retries`, `opt.retry_base_ms` and `opt.request_timeout_ms` run parameters. CON-29 requires each to be recorded in a required session attribute of its own. They are `acn.harness.endpoint`, `acn.harness.max_retries`, `acn.harness.retry_base_ms` and `acn.harness.request_timeout_ms`.
- The two `_ms` options are floats, because TRC-20's suffix rule types every `_ms` attribute as a float. Their defaults are `500.0` and `600000.0`.
- The inventory's own test requires every name it lists to appear in SPEC 010, which is the source of names. TRC-10 is therefore amended, in this PR, to name the four attributes. The spec line and the inventory entries have to land together: either one alone fails `cargo test`.

## Consequences
- `acn harness run` produces bundles on the mock in `sim` and against any endpoint in `live`. Real providers need a `real-api` build and credentials. The `live` tier tests are `#[ignore]` and run by hand.
- `engine_hash` moves, because the inventory changed.
- One finding is for the maintainer. MLM-21 reads only prefixes that the request marks. A single moving `rolling_tail` breakpoint is therefore never read again: on the mock it reads exactly what `system_only` reads (the knob test pins this). Anthropic's cache also looks back over earlier block boundaries before a breakpoint, which is what makes a rolling tail pay off there. Whether MLM-21 should model that look-back, or HAR-16 should mark the previous tail as well, is a SPEC 030 / SPEC 100 question. It decides whether `rolling_tail` can show any effect on `mock-explicit`.
