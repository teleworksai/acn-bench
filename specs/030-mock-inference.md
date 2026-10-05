# SPEC 030 — Mock inference: a deterministic model of prefill, decode and prompt caching

**Status:** Draft v0.2 (October 2026; v0.2: the reply policy honours a request's `tool_choice` of `none` and of `allowed_tools`, issues #24, #25). **Inherits:** SPEC 000, 010. **Prefix:** MLM. **Crate:** `acn-mockllm` (Class B).
**Purpose:** define a deterministic, OpenAI-compatible inference server whose latency and cache accounting follow stated rules, so that the harness (SPEC 040), the generator (SPEC 050) and every POC suite can run end to end without a provider, in `sim` (in process, virtual time) and in `live` (real sockets, wall time) — and so that a result obtained on it is visibly a test of the harness against our own model of caching, never a measurement of a provider (CON-26, PLAN §8b).

## 0. What the mock is for, and what it is not

The mock exists so that tests and the L1 loop (SPEC 085) can exercise the harness's cache discipline knobs against known mechanics, and so that the substrate runs without credentials or GPUs. It reproduces three cache mechanics that real providers use — explicit breakpoints, automatic prefix matching, block-granular prefix caching — as named, parameterised rules. It does not model any provider's numbers: a profile's constants are placeholders until an L3 calibration (SPEC 085) fits them to measurements, and even then a mock bundle stays mock-gated (CON-26, HYP-23).

## 1. Definitions

- **Profile** — a named, complete set of mock parameters (§5): cache model, timing constants, reply policy, faults. The `model` a request names is the profile name; it is also the `model` run parameter of CON-29.
- **Prompt bytes** — the canonical byte string of a request's prompt (MLM-10). **Tokens** — the prompt bytes split into 4-byte units (MLM-11).
- **Prefix** — the first *n* tokens of a prompt. **Cache entry** — a prefix the mock has stored, with its last-use time.
- **Tenant** — the cache namespace a request belongs to: the value of its `Authorization` header, or `-` without one. Requests of different tenants never share cache entries.
- **Virtual time / wall time** — time on the run's `Clock` (CON-5(b)) in `sim` and in `live` respectively. All mock times are integer nanoseconds on that clock.

## 2. Interface

**MLM-1** The mock MUST serve the OpenAI Chat Completions interface: `POST /v1/chat/completions` with `model`, `messages` (roles `system`, `user`, `assistant`, `tool`; `content` as a string or an array of `{type: "text", text}` parts; assistant `tool_calls`; tool `tool_call_id`), optional `tools` (`{type: "function", function: {name, description, parameters}}`), `max_tokens` or `max_completion_tokens`, `stream`, and `stream_options.include_usage`; and `GET /v1/models`, which lists the profiles. Any other field MUST be accepted and ignored, except `cache_control` (MLM-21). A request it cannot parse, or naming an unknown profile, MUST get a 400 with an OpenAI-shaped error body.

**MLM-2** A non-streamed response MUST be a `chat.completion` object with one choice whose `finish_reason` is `stop`, `tool_calls` or `length`, and a `usage` object carrying `prompt_tokens` (the total prompt length, cached or not), `completion_tokens`, `total_tokens`, and `prompt_tokens_details` with both `cached_tokens` and `cache_write_tokens`, always present, `0` when none. These are the fields the frozen provider mapping for `mockllm` reads (TRC-21, `acn_attributes.toml`); a change to either side is a change to both.

**MLM-3** A streamed response MUST be Server-Sent Events of `chat.completion.chunk` objects, one per output token, then a final chunk with the `finish_reason`, then — only when the request set `stream_options.include_usage` — a chunk with empty `choices` and the `usage` object of MLM-2, then `data: [DONE]`. Assembled as TRC-21 states (per-choice deltas accumulated, `usage` from the final chunk), a stream MUST yield the same content, `finish_reason` and `usage` as the non-streamed response to the same request in the same state.

**MLM-4** The mock MUST identify itself in every response (CON-26): an `x-acn-mockllm` HTTP header whose value is `acn-mockllm/<crate version> profile=<profile>`, and a `system_fingerprint` field of `acn-mockllm:<profile>` in every response object and stream chunk. A harness that sees either MUST record `acn.backend = "mockllm"` whatever it was configured with (CON-26; SPEC 040).

**MLM-5** The mock MUST be usable in two ways with one implementation of every rule below: in process, as a library call that takes a request, its tenant, its arrival time and the mock's state and returns the response, the times at which each output token is emitted, and the next state, with no socket and no clock read (`sim`); and as an HTTP server that applies the same call to requests arriving on real sockets and emits each token at its stated time on the wall clock (`live`). Both MUST produce byte-identical bodies and identical token times for the same request sequence, arrival times and seed.

## 3. Determinism

**MLM-6** Every random draw MUST come from the sub-stream `mockllm` (CON-30(b)) of the replicate the requests belong to, through the `ChaCha20Rng` of CON-5(a), and every time from the run's `Clock` (CON-5(b)). Timing arithmetic MUST be integer nanoseconds; no floating-point value MAY enter a time, a token count or a cache decision.

**MLM-7** In `sim`, the same profile, seed, request sequence and arrival times MUST produce byte-identical responses, identical token times and identical final state, on every run of the same build (CON-5(c)). Concurrent requests MUST be ordered by arrival time, then by tenant, then by the BLAKE3 of their prompt bytes, before any of them is processed, so that the order a scheduler happens to deliver them in cannot change a result.

**MLM-8** Each mock instance MUST start with an empty cache. A run that needs isolation between arms or replicates gets it from tenants or from separate instances, never from timing; which isolation a POC requires is stated by its spec (SPEC 040, SPEC 100).

## 4. Prompt bytes and tokens

**MLM-10** The prompt bytes of a request MUST be the concatenation, in the profile's `prefix_order` (default `tools`, `system`, `messages`), of: the canonical JSON of each tool definition in the order given; the canonical JSON of each `system` message; and the canonical JSON of every other message in order. Canonical JSON is UTF-8 with keys sorted bytewise, no insignificant whitespace, numbers in the text form of CON-27(c) (a tool schema's `"minimum": 0.5` is written `0.5`; a non-finite number gets a 400), and with every `cache_control` member removed, so that marking a breakpoint never changes the bytes. Each element is followed by one `\n`.

**MLM-11** A prompt of *b* bytes MUST have ⌈*b*/4⌉ tokens, token *i* being bytes 4*i*..4*i*+4 (the last one shorter). A prefix of *n* tokens is the first min(4*n*, *b*) bytes. The library MUST export this tokenizer, so that a harness on the mock counts `acn.call.new_input_tokens` with the `tokens` method on the same token sequences (TRC-12).

## 5. Cache models

**MLM-20** A profile MUST choose exactly one cache model: `explicit_breakpoints`, `automatic_prefix` or `block_granular`. In all three, a cache read is the length in tokens of the longest stored prefix of the same tenant that the request's prompt begins with, subject to the model's granularity; an entry that is read or written has its last-use time set to the request's arrival time; an entry whose last use is older than the profile's `ttl_ns` at a request's arrival is expired before the request is looked up; and `cached_tokens` never exceeds `prompt_tokens`.

**MLM-21** `explicit_breakpoints` (the mechanics of Anthropic's `cache_control`): a breakpoint is a `cache_control` member on a content part, on a message, or on a tool definition; it marks the prefix that ends with that element. At most `max_breakpoints` (default 4) are honoured, the first ones in prompt order; more MUST get a 400. A breakpoint whose prefix is shorter than `min_cacheable_tokens` is ignored. The read is the longest marked prefix that is stored; every marked prefix that is not stored is written, and `cache_write_tokens` is the length of the longest prefix written by the request. Nothing is cached without a breakpoint.

**MLM-22** `automatic_prefix` (the mechanics of OpenAI's automatic prompt caching): every request of at least `min_cacheable_tokens` stores its prompt rounded down to a multiple of `increment_tokens` (default 128) above `min_cacheable_tokens` (default 1024); the read is the longest stored prefix that matches, rounded down the same way; `cache_write_tokens` is always 0.

**MLM-23** `block_granular` (the mechanics of vLLM and SGLang prefix caching): prompts are cached in blocks of `block_tokens` (default 16) tokens; the read is the number of leading full blocks already stored, times `block_tokens`; every full block of the prompt is stored; `cache_write_tokens` is the number of blocks newly stored, times `block_tokens`. The store holds at most `capacity_blocks`; when full, the block with the earliest last use is evicted, ties broken by the lowest block hash, and a block is evicted only once no stored block extends it (leaves first, as a radix tree evicts). Block identity MUST be the BLAKE3 of the prefix bytes it ends, so that equal prefixes share blocks across requests and tenants never do (the tenant is part of the hash).

## 6. Timing

**MLM-30** A request MUST wait in a FIFO queue when `slots` (default unlimited) requests are already in service; `queue_ns` is the time it waits. Its time to first token MUST be `queue_ns + prefill_base_ns + prefill_ns_per_new_token × (prompt_tokens − cached_tokens) + prefill_ns_per_cached_token × cached_tokens`, and output token *k* (from 1) MUST be emitted at first-token time plus `(k − 1) × itl_ns` plus a jitter drawn per token, uniform over the integers in [−`itl_jitter_ns`, `itl_jitter_ns`], with no token emitted before the previous one. A streamed and a non-streamed response to the same request have the same times; the non-streamed body is sent at the last token's time.

**MLM-31** The mock MUST return its timing in an `x-acn-mock-timing` header (`queue_ns`, `prefill_ns`, `decode_ns`, as `key=value` pairs) on every response, so that a test can check MLM-30 against what a client measured, and so that a harness on the mock can be audited; a harness MUST NOT copy it into `acn.server.*`, which TRC-18 reserves for external nodes' own spans.

## 7. Replies

**MLM-40** A reply MUST be decided by the profile's reply policy from the request alone: with `tools` present and fewer than `tool_calls_per_turn` (default 1) tool results since the last `user` message, the reply MUST be one tool call to the tool at index (number of those results) modulo the number of tools, with arguments `{}` and an id derived from the BLAKE3 of the prompt bytes, and `finish_reason = "tool_calls"`; otherwise a text answer with `finish_reason = "stop"`. The answer's length in tokens MUST be drawn uniformly from the profile's `[output_tokens_min, output_tokens_max]`, capped by the request's token limit, in which case `finish_reason = "length"`. Answer text MUST be made of 4-byte words drawn from a fixed list in the crate, so that its token count by MLM-11 equals `completion_tokens`. A request's `tool_choice` MUST constrain this policy, and MUST NOT enter the prompt bytes (MLM-10).
- `"none"`, or `{"type": "none"}`, makes the reply a text answer whatever the tools.
- `{"type": "allowed_tools", "allowed_tools": {"tools": [...]}}` restricts the choice to the listed names that appear in `tools`, kept in `tools`' order: the reply calls the allowed tool at index (number of those results) modulo the number of allowed tools. When none of the listed names appears in `tools`, the reply is a text answer.
- Any other `tool_choice`, or none, leaves the policy as stated above.

**MLM-41** A profile MAY inject faults, each with a rate in parts per million drawn per request from the `mockllm` sub-stream: `429` with a `retry-after` in whole seconds, `500`, and a stream cut after a drawn number of tokens without a final chunk. Faults default to off, and a fault MUST NOT change the cache state.

## 8. Profiles

**MLM-50** Profiles MUST be data: a `profiles.toml` embedded in the crate, parsed with unknown keys rejected, every parameter of §5–§7 stated explicitly for every profile (no default is implied at load time), and the file's BLAKE3 reported by `GET /v1/models` and by the library. The crate MUST ship at least `mock-explicit` (`explicit_breakpoints`), `mock-auto` (`automatic_prefix`) and `mock-blocks` (`block_granular`), whose constants are labelled placeholders until an L3 calibration (SPEC 085) replaces them.

**MLM-51** Because the profiles are compiled into the binary, a change to them changes `build_hash` (CON-31) but not `run_id`, which carries only the profile name (CON-29); a verdict refuses a bundle set that mixes builds (HYP-20). A profile MUST NOT be renamed or reused for different constants: a new set of constants is a new name.

## 9. Bundles

**MLM-60** A bundle produced against the mock MUST carry `backend = "mockllm"` and the profile name as `model` (CON-26, CON-29); in `sim` it has no `endpoint_host`, in `live` it records the host the mock listened on (TRC-22). The mock emits no spans of its own in `sim`; in `live` it MAY emit an OTel server span per request under the caller's `traceparent`, as an external node would (TRC-18).

## 10. Acceptance tests (names are normative)

- `crates/acn-mockllm/tests/wire.rs` — MLM-1..4: request and response shapes, the stream assembling to the non-streamed response, the frozen `mockllm` mapping of TRC-21 normalising a response, the identity header and fingerprint.
- `crates/acn-mockllm/tests/tokens.rs` — MLM-10, MLM-11: canonical prompt bytes, `cache_control` not changing them, numbers in one text form, the tokenizer.
- `crates/acn-mockllm/tests/cache_models.rs` — MLM-20..23: each model on hand-computed request sequences, including TTL expiry, breakpoint limits, rounding, eviction order and tenant isolation.
- `crates/acn-mockllm/tests/timing.rs` — MLM-30, MLM-31: time to first token, token cadence and jitter bounds, queueing with a finite `slots`, the timing header.
- `crates/acn-mockllm/tests/determinism.rs` — MLM-5..8: the library and the HTTP server agree; two runs are byte-identical; delivery order does not matter.
- `crates/acn-mockllm/tests/replies.rs` — MLM-40, MLM-41: the reply policy and the fault model.
- `crates/acn-mockllm/tests/profiles.rs` — MLM-50: the shipped profiles load, state every parameter, and a malformed profile is refused.

## 11. Open questions (ADR candidates)

1. Whether a profile's BLAKE3 should enter `run_id` as an `opt.` parameter (CON-29), so that a recalibrated profile changes the identity of the runs that used it rather than only their `build_hash`.
2. Whether `prefix_order` should differ by cache model by default (Anthropic orders tools before the system prompt; the OpenAI order is not documented).
3. Real tokenizers group bytes very differently from MLM-11; whether a profile should carry a bytes-per-token ratio so that token counts are closer to a real model's, at the cost of the simple 4-byte rule.
