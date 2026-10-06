# SPEC 040 — Harness: a minimal agent loop with switchable cache-discipline knobs

**Status:** Draft v0.3 (October 2026; v0.3: HAR-41 batches calls by the instant they reach the mock, which a scenario delays to the uplink delivery, SPEC 020 EMU-33; v0.2: the compaction call refuses tool calls, and a forked sub-agent may call only its own tools, so that neither turns on a provider's or the mock's choice of tool; issues #24, #25). **Inherits:** SPEC 000, 010, 030; reads SPEC 080 (run parameters, control) where noted. **Prefix:** HAR. **Crate:** `acn-harness` (Class B), with the `acn harness` subcommand in `acn-cli`.
**Purpose:** define the smallest agent harness that does what real coding and retrieval harnesses do to a model's context — a system prompt, tool definitions, tool results, sub-agents and compaction — with each cache-relevant habit of such harnesses exposed as a named knob, so that POC 4 (SPEC 100, `hypotheses/p4.toml`) can vary those habits one at a time against the mock (SPEC 030) and against real providers, and record what each costs in the trace schema of SPEC 010.

## 0. What the harness is for, and what it is not

The report's claim (§3.6) is that the harness, not the model or the serving layer, decides cacheability. Testing it needs a harness whose context-building is fully specified, so that two arms differ in exactly one habit and in nothing else. This harness is therefore not a useful agent: its tools are simulated (HAR-2), its tasks are scripted (HAR-60), and success is decided by a checker over what it did, not by the quality of an answer (HAR-3). What it reproduces faithfully is the byte stream real harnesses send, and the cache accounting that stream earns.

## 1. Definitions

- **Workload** — the file of HAR-60: the agent's system prompt and call parameters, its tool table, and its tasks. Its hash is `workload_hash` (CON-27a).
- **Task** — a scripted sequence of user turns; one task is one `acn.session` per replicate. **Turn** and **call** are as in SPEC 010.
- **Context** — what the harness sends on one call: the system prompt, the tool definitions and the messages, in the order the dialect (HAR-21) puts them, and the call's tool choice when HAR-4 or HAR-14 sets one. A **lineage** is as in TRC-12: the session's main chain, or one sub-agent.
- **Knob** — a named cache-discipline habit with a closed domain (HAR-10). The **knob map** is the value of every knob in force for a run.
- **Dialect** — the wire format of a backend: `chat_completions` (OpenAI Chat Completions; `mockllm`, `openai`, `vllm`, `sglang`) or `messages` (Anthropic Messages; `anthropic`).
- **Isolation marker** — the per-replicate, per-arm string of HAR-42 that keeps caches of different replicates and arms apart.

## 2. The agent loop

**HAR-1** For each task of the workload, in order, the harness MUST run one session. For each turn of the task it MUST append the turn's user message (HAR-13 may prefix it) and then repeat: build the context under the knob map, make one call, and, if the response asks for tools, run every requested tool in the order given, append the assistant message and one tool-result message per tool, and call again; the turn ends when a response asks for no tool, when the workload's `max_calls_per_turn` is reached (outcome `aborted`), when a call fails after its retries (HAR-24; outcome `aborted`), or when the turn's deadline, if it has one, passes (outcome `timeout`). Between turns the harness MUST wait the turn's think time on the run's `Clock` (HAR-40).

**HAR-2** Tools MUST be simulated from the workload's tool table: the harness never reads a file, runs a command or makes a network request on a tool's behalf. Each tool has a class (TRC-13), a result length in bytes and a duration in nanoseconds, each drawn uniformly from its declared integer range, and a result made of printable ASCII drawn from a fixed alphabet in the crate; every draw comes from the sub-stream `harness.tools` (HAR-40). A tool's duration elapses on the run's `Clock` before its result is appended.

**HAR-3** A turn's outcome MUST be decided by the workload's checker and never by the model or by the harness's own judgement (TRC-11): `success` when the turn ended because a response asked for no tool and, if the turn lists `expect_tools`, every listed tool was called during the turn; `failure` when it ended that way otherwise. `aborted` and `timeout` are as HAR-1 states. The checker is part of the workload, so it is hashed with it.

**HAR-4** Compaction: before each call on the main lineage the harness MUST evaluate the compaction trigger in force (HAR-15). When it fires, the harness MUST first make a compaction call — the current context followed by a user message carrying the workload's `summary_instruction`, with the token limit `summary_max_tokens` — and then replace the context's messages by one user message holding the summary text, followed by the messages of the current turn. A compaction call is a `chat` span of the lineage like any other, and the turn records the trigger that fired in `acn.turn.compaction` (`none` when none did). At most one compaction MAY happen per call. The compaction call MUST carry the context's tools unchanged, so that its prefix is the context's. When the context has tools, it MUST forbid tool calls: `"tool_choice": "none"` on the chat-completions dialect, `"tool_choice": {"type": "none"}` on the messages dialect. A context without tools carries no tool choice. A summary is text, and a compaction call that cannot answer with text would leave an empty one.
- **On the mock**, a compaction call reads the whole marked prefix: MLM-21 does not key on `tool_choice`.
- **On `anthropic`**, a change of tool choice may invalidate the cached message blocks while the tools and system stay cached. A compaction call there can therefore read less than the mock's. SPEC 100 states this with the `compaction_trigger` effect.

**HAR-5** Fan-out: a tool of class `subagent` declares a `width` and a child specification (instruction, tools, `max_calls`). When a response calls it, the harness MUST spawn `width` sub-agents concurrently, each an `invoke_agent` (TRC-14) child of the tool's `execute_tool` span, each running the loop of HAR-1 for one turn in its own lineage with the context HAR-14 gives it, and MUST return their final answers, concatenated in child order, as the tool's result. Sub-agents MUST NOT spawn sub-agents in this version (`acn.fanout.depth` is 1).

## 3. Knobs

**HAR-10** The knobs MUST be exactly the six below, with these domains and these defaults; the defaults are the configuration the harness ships with, which is the control of `hypotheses/p4.toml`. A run takes each knob's value from its `vary.<knob>` run parameter (CON-29) when present and the default otherwise; a knob value outside its domain MUST be refused before the run starts. A `vary.<name>` that is not a knob (`provider`, `workload` in `hypotheses/p4.toml`) is recorded as a run parameter and changes nothing in the harness; with a hypothesis file it MUST be one of the file's `[varies]` parameters, and without one only knob names are accepted, so that a misspelt knob cannot pass silently. The knob map MUST be recorded on every session as `acn.harness.knobs`, a JSON object with all six keys, sorted, with no insignificant whitespace (TRC-10).

| Knob | Domain | Default |
|---|---|---|
| `timestamp_in_system_prompt` | bool | `true` |
| `tool_order_stable` | bool | `true` |
| `backfill_mode` | `tail_restate`, `mid_prefix` | `mid_prefix` |
| `fanout_prompting` | `fork_from_prefix`, `per_child` | `per_child` |
| `compaction_trigger` | `window_full`, `read_cost_threshold` | `window_full` |
| `cache_breakpoint_placement` | `none`, `system_only`, `system_and_tools`, `rolling_tail` | `system_only` |

**HAR-11** `timestamp_in_system_prompt`: when true, the system prompt MUST carry, on the line after the isolation marker (HAR-42), `Current time: <t>`, where `t` is the start time of the current turn on the run's `Clock` in whole milliseconds; when false, that line MUST be absent. The value therefore changes at every turn, as a harness that stamps the wall-clock time does.

**HAR-12** `tool_order_stable`: when true, every request MUST present the tool definitions in workload order; when false, every request MUST present them in a fresh uniformly random permutation drawn from the sub-stream `harness.knobs`, as a harness that builds its tool list from an unordered map does.

**HAR-13** `backfill_mode`: a turn MAY list `updates`, earlier tool results of the session (by their ordinal in the session, from 0) whose content changed before the turn; the new content is drawn as HAR-2 draws a result. Under `mid_prefix` the harness MUST replace the content of each such tool-result message in place. Under `tail_restate` it MUST leave every earlier message unchanged and MUST prefix the turn's user message with, for each update in order, `Updated result <ordinal>:\n<content>\n`.

**HAR-14** `fanout_prompting`: under `fork_from_prefix`, a sub-agent's first context MUST be the parent's context as it was sent on the call that spawned it, without that call's response, followed by one user message holding the child instruction and the child's index; under `per_child`, it MUST be the child specification's own system prompt, the child's tools and one user message holding the instruction and index. `acn.fanout.shared_prefix_tokens` MUST be the length, by the run's counting method (HAR-32), of the common prefix of the child's first request and the spawning request. Under `fork_from_prefix` the child keeps the parent's tool definitions, which are part of the shared prefix, but MAY call only its specification's tools. Every request of the child MUST say so:
- its instruction message MUST end with the line `Use only these tools: <names>.`, the names comma-separated in the specification's order;
- on `openai` and `mockllm`, every request MUST also carry `"tool_choice": {"type": "allowed_tools", "allowed_tools": {"mode": "auto", "tools": [...]}}`, listing those tools as `{"type": "function", "function": {"name": <name>}}` in the specification's order.

The instruction line is the last line of the message, after `(child <i> of <w>)` and one `\n`, with no newline after it.
- **Backends without the restriction.** The messages dialect (`anthropic`) and the `vllm` and `sglang` backends, whose servers do not accept `allowed_tools`, carry the instruction line only. There the restriction rests on the model, and SPEC 100 says so where it compares providers.
- **A child with no tools** gets the line `Use no tools.` and a tool choice of `none` in its dialect's form (HAR-4).
- **Enforcement.** The child lineage presents the parent's tools but MUST execute only its specification's. A call to any other tool gets the result `error: tool '<name>' is not available here`, as a call to an unknown tool does.

Under `per_child` the child has only its own tools, and neither the line nor a tool choice is sent.

**HAR-15** `compaction_trigger`: under `window_full`, the trigger MUST fire when the estimated length of the request about to be sent, ⌈canonical bytes / 4⌉ by the canonicalisation of TRC-12, is at least the workload's `compact_at_tokens`, an estimate that is the same for every backend; under `read_cost_threshold`, when the previous call of the lineage reported uncached input tokens (`acn.call.input_tokens − acn.cache.read_tokens`) above the workload's `read_cost_threshold_tokens`, which by design depends on what the provider cached.

**HAR-16** `cache_breakpoint_placement`: the harness MUST mark breakpoints (a `cache_control` of type `ephemeral`) only on the `messages` dialect and on the `mockllm` backend, and MUST NOT send `cache_control` to any other backend. `none` marks nothing; `system_only` marks the last system block; `system_and_tools` marks the last tool definition and the last system block; `rolling_tail` marks the last system block and the last content block of the request's last message.

**HAR-17** A knob MUST change nothing but what its clause states: for the same seed and workload, two runs whose knob maps differ in one knob MUST send requests that are byte-identical, apart from the isolation marker (HAR-42), wherever that knob's clause does not apply (for example, every request of a run on `openai` is the same under all four breakpoint placements).

## 4. Backends and the wire

**HAR-20** The harness MUST support the backends `mockllm`, `openai`, `vllm` and `sglang` on the `chat_completions` dialect and `anthropic` on the `messages` dialect. On `mockllm` in `sim` it MUST call `acn_mockllm::Mock` in process (MLM-5) and open no socket; everywhere else it calls an HTTP endpoint. Every code path that can reach a real provider MUST be compiled only with the feature `real-api`; a default build can reach only an endpoint the operator names, and the tests of the default tiers only the mock.

**HAR-21** Requests MUST follow each dialect's published format: on `chat_completions`, the fields of MLM-1; on `messages`, a top-level `system` array of text blocks, `tools` with `input_schema`, and `tool_use` / `tool_result` content blocks, with the API version header pinned in the crate. The order of the context is tools, system, messages on both. A request body MUST be serialised deterministically (fields in a fixed order, numbers in the text form of CON-27(c)), so that its bytes are a function of the context, the tool choice included, and the knob map alone. Sampling parameters, the token limit and `stream` come from the workload (HAR-60); streaming is the default.

**HAR-22** Credentials MUST come from environment variables, MUST NOT enter a request anywhere but the authentication header, and MUST NOT be written to a bundle, a log or an error message.

**HAR-23** Backend identity (CON-26, MLM-4): before the first session of a `live` run, the harness MUST request `GET /v1/models` from the endpoint, and it MUST inspect every response for the `x-acn-mockllm` header or a `system_fingerprint` beginning `acn-mockllm:`. If the run was configured with a backend other than `mockllm` and either marker is seen, or with `mockllm` and the probe shows no marker, the run MUST stop with `"ok": false` and the error `backend_mismatch`, and MUST NOT write a bundle, because its `run_id` (CON-29) was derived from the configured backend. Otherwise the configured backend is the one recorded in `acn.backend` and in the manifest.

**HAR-24** Retries: a 429, a 5xx or a transport error MUST be retried up to `opt.max_retries` times, waiting the response's `retry-after` when it has one and otherwise `opt.retry_base_ms × 2^k` before retry *k* (from 0), on the run's `Clock`; each retry MUST be counted in `acn.call.retries`. A call that still fails ends with `acn.call.error_class` and `acn.call.stop_reason = transport_error`, and its turn ends `aborted`. A call that exceeds `opt.request_timeout_ms` MUST be abandoned as `client_abort`. A stream cut before its final chunk (MLM-41) is a transport error.

**HAR-25** The options of HAR-24, and `opt.endpoint` (the endpoint's URL), MUST be `opt.` run parameters (CON-29) listed with their defaults in `acn_attributes.toml`; adding them there is a Class C change made by the implementing PR. No other option and no environment variable other than credentials and logging MAY change what the harness sends or how it behaves.

## 5. What it records

**HAR-30** The harness MUST emit, through `acn-trace` as the producer `acn-harness`, one `acn.session` per task and replicate, one `acn.turn` per turn, one `chat` per call (compaction calls included), one `execute_tool` per tool run and one `invoke_agent` per sub-agent, with the attributes TRC-10 to TRC-14 require. `acn.role` is the run's arm. `acn.tool.placement` is `local`, and `acn.tool.requesting_call` is the index of the `chat` whose response asked for the tool.

**HAR-31** Usage MUST be normalised by the frozen per-provider mapping of TRC-21 (`acn_trace::normalise`) applied to the provider's response, and by no mapping of the harness's own; the harness records the provider's raw usage fields as the corresponding `gen_ai.*` attributes and the normalised values as `acn.call.input_tokens`, `acn.call.output_tokens`, `acn.call.stop_reason`, `acn.cache.read_tokens` and `acn.cache.write_tokens`. A count the provider did not return MUST be absent, never invented (TRC-12).

**HAR-32** `acn.call.new_input_tokens` MUST use the `tokens` method on `mockllm`, over the prompt bytes and tokenizer the mock exports (MLM-10, MLM-11) for the profile named by `model`, and the `bytes_scaled` method of TRC-12 on every other backend; one run uses one method.

**HAR-33** Timing MUST be read from the run's `Clock`: `acn.call.ttft_ms` from the moment the request is handed to the transport to the first chunk carrying content or a tool call (for a non-streamed call, to the response, as TRC-12 says), ITL quantiles over the gaps between such chunks, and one `acn.stream.stall` per gap above `opt.stall_threshold_ms`. `acn.call.wire_bytes_up` and `acn.call.wire_bytes_down` are the request and response body lengths in bytes, as sent and as received. The mock's `x-acn-mock-timing` header MUST NOT be copied into any attribute (MLM-31).

**HAR-34** Message content MUST NOT appear in any span, event or view (TRC-42); the harness records lengths and token counts only.

## 6. Determinism and isolation

**HAR-40** Every draw MUST come from a sub-stream of the replicate (CON-30(b)): `harness.tools` for tool results and durations, `harness.knobs` for tool permutations, `harness.workload` for think times; the mock in process draws from its own `mockllm` sub-stream (MLM-6), and span ids from `trace.ids` (TRC-27). Every time MUST come from the run's `Clock`.

**HAR-41** In `sim` the harness MUST run on a `SimClock` against the in-process mock. Calls that reach the mock at the same instant MUST be submitted together through `Mock::handle_batch`, so that their order is MLM-7's and never the scheduler's. Without a scenario a call reaches the mock when it is made; with one, at its request's uplink delivery time, and the instant's phases are SPEC 020 EMU-33's. Two sim runs with the same inputs MUST produce byte-identical bundles (CON-5(c), TRC-24).

**HAR-42** Isolation: each (run, arm, replicate) MUST have an isolation marker, the first 16 hex digits of `blake3("acn-bench/harness_isolation/v1\0" ‖ run_id ‖ arm ‖ i)` (CON-27, `arm` as a string and `i` as an unsigned 32-bit integer), written as the first line of the system prompt, `Session: <marker>`, and at the start of every tool definition's description. No cached prefix can then be shared between replicates or arms, on any provider and in either cache order, while the sessions of one replicate still share theirs, as the sessions of one user do. On `mockllm` the tenant (the `Authorization` header) MUST also be the marker.

**HAR-43** Within a run, replicates MUST execute in the permutation of their indices drawn from the sub-stream `run.order` and recorded as the manifest's `execution_order` (CON-5(d)); the tasks of one replicate run in workload order.

## 7. Runs and bundles

**HAR-50** `acn harness run` MUST execute one cell and one arm — `--workload <file>`, `--backend`, `--model`, `--mode sim|live`, `--arm treatment|control`, `--replicates <n>`, `--vary <name>=<value>` per varied parameter (HAR-10), the `opt.` options of HAR-25, and either `--hypothesis <file>` or, for hypothesis `none` only, `--seed` (HYP-9) — into one bundle under `runs/<run_id>/`, and MUST print one JSON object with `ok`, `run_id` and `bundle_digest` (CON-8, TRC-23).

**HAR-51** The manifest MUST carry the run parameters of CON-29 (`backend`, `model`, `hyp_status`, `arms`, `replicates`, one `vary.<name>` per parameter given, and every `opt.` that differs from its default), `endpoint_host` for every backend except `mockllm` in `sim` (CON-26, MLM-60), and the workload's hash; a bundle on the mock carries `backend = "mockllm"` and the profile name as `model`.

**HAR-52** `sim` mode MUST be refused for any backend but `mockllm`, since it has no sockets; `netem` is out of scope until SPEC 020 defines it.

## 8. Workloads

**HAR-60** A workload MUST be a TOML file under `workloads/`, parsed with unknown keys rejected at every level and every parameter stated (nothing implied at load time): `schema_version = 1`; an `[agent]` table (`system_prompt`, `temperature`, `max_tokens`, `stream`, `max_calls_per_turn`, `compact_at_tokens`, `read_cost_threshold_tokens`, `summary_instruction`, `summary_max_tokens`); one `[[tool]]` per tool (`name`, `class`, `description`, `parameters` as a JSON-schema table, `result_bytes` and `duration_ns` as `{ min, max }`, and for class `subagent` `width` and `child`); and one `[[task]]` per task with its `[[task.turn]]`s (`user`, `think_time_ns` as `{ min, max }`, and optionally `deadline_ms`, `expect_tools`, `updates`). A workload that reads another file MUST name it with its BLAKE3 (CON-27(a)). `workloads/` MUST be added to `.gitattributes` as `-text`. A `subagent` tool's child tools MUST be among the tools of every task that lists the `subagent` tool, so that under `fork_from_prefix` each child tool is in the tools the child presents; a workload that breaks this MUST be refused.

**HAR-61** The crate's tests MUST use a small workload, `workloads/harness-smoke.toml`, that exercises every clause of §2 and §3 in a few calls. The workloads of a POC are defined by its spec (SPEC 100 for POC 4).

## 9. Acceptance tests (names are normative)

- `crates/acn-harness/tests/agent_loop.rs` — HAR-1..5: turn termination and outcomes, simulated tools, the checker, compaction (its tool choice, a text summary on the mock, none on a tool-less context), fan-out (forked children calling only their own tools).
- `crates/acn-harness/tests/knobs.rs` — HAR-10..17: each knob's effect on the request bytes (HAR-14's instruction line and `allowed_tools` per backend, neither under `per_child`, enforcement), the knob map attribute, and the direction each knob moves `cached_tokens` on each of `mock-explicit`, `mock-auto` and `mock-blocks`; a knob that does not apply changes no byte.
- `crates/acn-harness/tests/wire.rs` — HAR-20..25: both dialects' encodings on golden contexts, the backend-mismatch refusal, retries and timeouts against the mock's faults, credentials absent from every output.
- `crates/acn-harness/tests/record.rs` — HAR-30..34: spans and attributes, the TRC-21 mapping, the counting method, timing on a `SimClock`, no content in spans.
- `crates/acn-harness/tests/isolation.rs` — HAR-40..43: sub-streams, simultaneous sub-agent calls, isolation markers, execution order.
- `crates/acn-harness/tests/workload.rs` — HAR-60, HAR-61: the workload format and its refusals, including a child tool missing from a task that lists its `subagent` tool.
- `tests/accept/harness_sim.rs` — HAR-50..52, TRC-24: a sim run of the smoke workload, twice, yields byte-identical bundles that `acn bundle verify --views` accepts.
- `crates/acn-harness/tests/live_providers.rs` — `live` tier (`#[ignore]`, `real-api`): one session of the smoke workload per real provider, with cached tokens recorded and normalised.

## 10. Open questions (ADR candidates)

1. **Success on a real model.** On the mock, `success` is a protocol property (the reply policy, MLM-40, decides which tools are called). On a real model, whether a scripted turn calls its `expect_tools` depends on the model, so `cost_per_success` mixes cache discipline with model behaviour. SPEC 100 should say whether POC 4's real-provider tasks need checkers that do not depend on tool choice.
2. **The isolation marker is visible to the model.** It isolates caches on every provider without provider-specific features, at the cost of a few tokens and of content a real model sees. OpenAI's `prompt_cache_key` is not sent; whether it should be is a provider-behaviour question for SPEC 100.
3. **Timestamp granularity.** Real harnesses stamp the date, the minute or the second; HAR-11 stamps the turn's millisecond, the most cache-hostile choice. A coarser stamp only weakens the effect within a turn, never across turns.
4. **Conditional domains.** `cache_breakpoint_placement` does nothing on automatic-prefix and block-granular backends (HAR-16, HAR-17), so its cells there are replicates of each other; SPEC 080 §6 question 1 leaves it to SPEC 100.
5. **Real tools.** Simulated tools make results independent of a file system or network. A POC that needs real tool latency would add a `placement = remote` tool class that calls an endpoint through the emulator (SPEC 020).
