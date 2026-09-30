# SPEC 010 — Trace schema: the ACN profile of OpenTelemetry

**Status:** Draft v0.2 (September 2026; v0.2 makes the span profile, bundle layout and views normative in wording, adds the report §3.5 fields that were missing — stop reason, new input tokens, preceding tool, think time — adds `build_hash`, `engine_hash` and a `bundle_digest` to the manifest, and adds the report-coverage requirement TRC-36). **Inherits:** SPEC 000. **Prefix:** TRC. **Crate:** `acn-trace` (schema module is in the frozen set, CON-7).
**Purpose:** define what every run of acn-bench records, in what shape, and how a bundle is laid out — such that (a) any tool that understands OpenTelemetry can read a run, (b) the §3.5 session/turn/call tables of the ACN report are *derived views* rather than a storage format, and (c) two runs with the same inputs produce byte-identical files in `sim` mode.

## 0. Design decision (ADR-2)

acn-bench does not define a trace format. It adopts **OpenTelemetry (OTel)** as the event model — spans, span events, attributes, W3C trace context — and adds an `acn.*` attribute namespace for what OTel does not cover. Reasons: inference engines (vLLM, SGLang) already emit OTel spans, so server-side attribution arrives as data rather than scraping; the GenAI semantic conventions are where agent-observability tooling has converged; W3C `traceparent` propagation gives the harness → proxy → endpoint chain one trace ID, so boundary timestamps fall out of context propagation; and standard viewers (Jaeger, Tempo, Grafana) can open a bundle without installing the kit.

Maturity is the known cost. As of July 2026 the GenAI conventions live in their own repository (`open-telemetry/semantic-conventions-genai`) and **no GenAI span, event, metric or attribute is marked Stable**; names have already moved once (`gen_ai.system` → `gen_ai.provider.name` in v1.37; `prompt_tokens`/`completion_tokens` → `input_tokens`/`output_tokens` in v1.27; cache-token attributes added in v1.40; `invoke_agent` split and stricter `execute_tool` naming in v1.41). This spec therefore **pins a convention version** (TRC-2) and keeps every quantity a POC verdict depends on under `acn.*`, so a convention rename never changes a verdict. Storage is Parquet via `arrow-rs` with a layout that mirrors OTLP; the upstream otel-arrow Parquet exporter was closed as not planned, so acn-bench owns its writer (TRC-20).

## 1. Definitions

- **Run** — one execution under one `run_id` (CON-5e). **Bundle** — the directory a run produces (§5).
- **Session / turn / call** — as in ACN report §3.1: a session is a harness process lifetime; a turn is one user-visible harness loop; a call is one model or tool invocation within a turn.
- **Boundary** — the point where the network is inserted (the `live` proxy or the `sim` link), where link segments are timestamped.
- **Producer** — any component emitting spans: `acn-harness`, `acn-gen`, `acn-emu` (proxy/sim), `acn-mockllm`, `acn-replay`, external inference nodes.

## 2. Conventions and versions

**TRC-1** All telemetry MUST be expressed as OpenTelemetry spans, span events and resource attributes as defined by the OTLP data model; producers MUST emit through the `opentelemetry` / `opentelemetry_sdk` Rust crates or, for external systems, standard OTLP.

**TRC-2** The schema MUST pin the semantic-conventions version it targets in `crates/acn-trace/src/schema/SEMCONV_VERSION` (initially the GenAI conventions at v1.41.0 and the general conventions at the matching release). A change of pinned version is a Class C change and MUST come with a mapping table for every renamed attribute the ingester (TRC-30) reads.

**TRC-3** Every attribute a hypothesis falsifier (SPEC 080) or an attribution statistic (SPEC 090) reads MUST be an `acn.*` attribute (§4), never a `gen_ai.*` or other convention attribute directly. Convention attributes are emitted for ecosystem compatibility and are mapped into `acn.*` at ingest (TRC-30).

**TRC-4** Trace context MUST propagate as W3C `traceparent`/`tracestate` over every HTTP hop on the run path, including through the `live` proxy and into external inference nodes when they accept it. In `sim` mode context propagation is in-process and MUST produce the same parent/child structure.

## 3. Span profile

Span names follow the GenAI conventions where they exist; `acn.*` names are used for what they do not cover. The attributes each span MUST carry are in addition to the convention's own. An attribute marked *optional* MUST be present when its stated condition holds and MUST be absent, never zero or empty, otherwise.

**TRC-10** `acn.session` (root, kind INTERNAL) — one per harness or generator session. MUST carry: `acn.run_id`, `acn.hypothesis.id`, `acn.hypothesis.status` (`candidate`|`frozen`), `acn.backend` (`mockllm`|`anthropic`|`openai`|`vllm`|`sglang`|…), `acn.mode` (`sim`|`live`|`netem`), `acn.scenario.hash`, `acn.workload.hash`, `acn.seed`, `acn.replicate`, `acn.role` (`treatment`|`control`|`researcher`, the last for LOOP-21 sessions), `acn.harness.knobs` (JSON string of the knob map in force, SPEC 040).

**TRC-11** `acn.turn` (child of session, kind INTERNAL) — one per harness turn. MUST carry: `acn.turn.index`, `acn.turn.deadline_ms` (optional: the turn has a deadline), `acn.turn.outcome` (`success`|`failure`|`timeout`|`aborted`; `success` and `failure` MUST be decided by the workload's checker, which is part of the hashed workload, and never by the model or the harness under test, because this field is the denominator of every per-success quantity), `acn.turn.first_useful_result_ms` (offset from turn start; optional: a useful result was produced), `acn.turn.compaction` (`none`|`window_full`|`read_cost_threshold`).

**TRC-12** `chat` (child of turn, or of the `invoke_agent` whose sub-agent makes the call; kind CLIENT) — one per model call, per GenAI conventions (`gen_ai.operation.name = "chat"`, `gen_ai.provider.name`, `gen_ai.request.model`, `gen_ai.usage.input_tokens`, `gen_ai.usage.output_tokens`, and the v1.40 cache-token attributes where the provider reports them). MUST carry `acn.*`: `acn.call.index` (counts the `chat` spans of one context lineage within one turn, from 0, in start order; a lineage is the session's main chain or one `invoke_agent`, so parallel sub-agents never share a counter), `acn.call.input_tokens` (the **total** prompt length in tokens, cached or not: providers differ, Anthropic's `usage.input_tokens` excludes cached tokens and OpenAI's `prompt_tokens` includes them, so the per-provider mapping of TRC-21 MUST produce the total; optional, like the other token counts: the provider returned usage, which a call that ends in `transport_error` or `client_abort` may never do, and a producer MUST NOT invent a count), `acn.call.new_input_tokens` (`input_tokens` minus the token length of the longest common prefix between this call's input and the previous call's input followed by its output, where *previous* means the previous `chat` in the same context lineage — the session's main chain, which continues across turns, or the enclosing `invoke_agent` — and, for the first call of a lineage, the parent's context at spawn time; never negative; equal to `input_tokens` when there is no previous context. Where the producer has the token sequences (the mock backend, a local tokenizer) it MUST use them; otherwise it MUST reduce each of the two requests to one canonical byte string — the system prompt, the tool definitions and the messages, in that order, each serialised by the producer's JSON serialiser, the previous response being compared as the message it became — take `lcp_bytes`, the length of their common prefix, and `request_bytes`, the length of this call's string, and compute `new_input_tokens = input_tokens − floor(lcp_bytes × input_tokens / request_bytes)` in unsigned 64-bit integer arithmetic, so that a tool block that never changes is never counted as new), `acn.call.new_input_tokens_method` (`tokens`|`bytes_scaled`, which of the two was used; a run uses one method throughout, and a verdict refuses a bundle set that mixes them, HYP-20), `acn.call.output_tokens`, `acn.call.stop_reason` (`end_turn`|`tool_use`|`max_tokens`|`stop_sequence`|`content_filter`|`client_abort`|`transport_error`|`other`; optional: the call ended with a provider stop reason or was cut by the client or the link; normalised per provider in the mapping table of TRC-21, where an OpenAI-compatible `stop` maps to `end_turn`) and `acn.call.stop_reason_raw` (the provider's own value; optional: the provider reported one), `acn.cache.read_tokens`, `acn.cache.write_tokens` (0 when the provider has no such concept), `acn.call.ttft_ms` (optional: at least one token arrived; for a non-streamed call it equals the total time, by design), `acn.call.itl_p50_ms` and `acn.call.itl_p99_ms` (optional: streamed, at least two tokens), `acn.call.wire_bytes_up`, `acn.call.wire_bytes_down`, `acn.call.streamed` (bool), `acn.call.retries`, `acn.call.error_class` (optional: the call failed). Span events: `acn.stream.first_token`, `acn.stream.last_token`, one `acn.stream.stall` event per gap > `acn.stall_threshold_ms` (default 250) with attributes `gap_ms`, `tokens_before`.

**TRC-13** `execute_tool` (child of turn, or of the `invoke_agent` whose sub-agent runs the tool; kind INTERNAL or CLIENT) — one per tool call, per GenAI conventions (`gen_ai.tool.name`, `gen_ai.tool.call.id`). MUST carry `acn.*`: `acn.tool.class` (`file`|`shell`|`search`|`http`|`subagent`|`testbed`|`other`), `acn.tool.result_bytes`, `acn.tool.placement` (`local`|`remote`), `acn.tool.requesting_call` (the `acn.call.index`, in the same lineage and turn, of the `chat` whose response asked for this tool: the harness knows it, and timestamps cannot recover it when a later call finishes before a delayed tool starts). A sub-agent spawned through a tool is an `execute_tool` of class `subagent` whose child is the `invoke_agent`.

**TRC-14** `invoke_agent` (child of turn, of another `invoke_agent`, or of the `execute_tool` of class `subagent` that spawned it; kind INTERNAL) — one per sub-agent spawn, per conventions. MUST carry: `acn.fanout.parent_turn`, `acn.fanout.parent_call` (the `acn.call.index` of the `chat` whose response spawned it), `acn.fanout.parent_lineage` (the `span_id` of the spawner's own `invoke_agent`; optional: the spawner is itself a sub-agent; with `parent_turn` and `parent_call` it identifies the spawning `chat` uniquely), `acn.fanout.depth` (1 for a child of the main chain), `acn.fanout.width` (the number of siblings spawned by the same parent call, this one included), `acn.fanout.shared_prefix_tokens`.

**TRC-15** `acn.link` (kind INTERNAL, emitted by `acn-emu`) — one per message or byte-segment crossing the boundary, parented to the call whose traffic it carries via propagated context (or by the proxy's flow map when the payload is opaque). MUST carry: `acn.link.id`, `acn.link.direction` (`up`|`down`), `acn.link.bytes`, `acn.link.enqueue_ns`, `acn.link.dequeue_ns`, `acn.link.applied_delay_ms`, `acn.link.dropped` (bool), `acn.link.reordered` (bool), `acn.link.rate_limited_ms`, `acn.link.model` (scenario link-model name). Span events on the link's parent scenario span: `acn.scenario.step` (`step`, `params` JSON), `acn.scenario.outage` (`start_ns`, `end_ns`, `cause` = `handover`|`scheduled`|`trace`).

**TRC-16** `acn.scenario` (root, kind INTERNAL, emitted by `acn-emu` once per run) — MUST carry the full scenario as `acn.scenario.toml` (string) and `acn.scenario.hash`; all `acn.link` spans link to it via a span link.

**TRC-17** `acn.replay.roundtrip` (emitted by `acn-replay`, child of session) — one per perception→plan→act cycle, which MUST carry: `acn.replay.frame_index`, `acn.replay.obs_age_ms`, `acn.replay.deadline_ms`, `acn.replay.deadline_missed` (bool), `acn.replay.fallback` (bool), `acn.replay.bitrate_kbps`, `acn.replay.score`.

**TRC-18** External inference-node spans (vLLM, SGLang) are ingested unmodified; the ingester (TRC-30) MUST attach them to the `chat` span whose `traceparent` they carry and MUST derive `acn.server.queue_ms`, `acn.server.prefill_ms`, `acn.server.decode_ms` from them when the node's conventions allow, else leave those attributes absent (never zero).

**TRC-19** Every producer MUST set resource attributes `service.name` (crate name), `service.version` (crate version), `acn.engine_hash` (CON-28), `acn.build_hash` (CON-31). A git commit identifier MUST NOT appear in any file covered by TRC-24: two builds of identical source from different commits would otherwise differ; it MAY be written under `logs/`.

## 4. The `acn.*` namespace

**TRC-20** The complete list of `acn.*` attributes, their types, units and producers MUST be maintained in `crates/acn-trace/src/schema/acn_attributes.toml`; `cargo xtask docs-inventory` MUST regenerate `docs/generated/acn-attributes.md` from it and fail if a producer emits an `acn.*` attribute not in the list. Units follow OTel conventions (`_ms`, `_ns`, `_bytes`, `_tokens` suffixes; booleans unsuffixed).

**TRC-21** Provider normalisation happens at ingest: `acn.cache.read_tokens`, `acn.cache.write_tokens`, `acn.call.input_tokens` (the total, TRC-12) and `acn.call.stop_reason` MUST each be populated from each provider's own fields by a per-provider mapping in `acn_attributes.toml`, which for stop reasons lists every provider value it knows and maps any other to `other` (Anthropic explicit-breakpoint fields, OpenAI automatic prefix fields, vLLM/SGLang prefix-cache metrics). The mapping MUST record the provider field name used, so a per-provider POC 4 verdict is auditable.

## 5. Bundle layout

**TRC-22** A bundle MUST be a directory `runs/<run_id>/` containing exactly:

```
manifest.json          run_id, seed (a decimal string, CON-30), mode, backend, model, endpoint_host (CON-26; absent for mockllm in sim),
                       scenario_hash, workload_hash, hypothesis {id,status,hash}, engine_hash (CON-28), build_hash and its components (CON-31),
                       params (the key/value pairs of CON-29), execution_order (CON-5d; live and netem), semconv_version, producers[],
                       started_at (wall, live only), replicates, files {path: blake3} (TRC-23)
spans.parquet          all spans (§6)
events.parquet         all span events
links.parquet          span links
resources.parquet      resource attribute sets, keyed by resource_id
views/session.parquet  derived (§7)
views/turn.parquet
views/call.parquet
views/link.parquet
views/tool.parquet
sidecar/               optional: *.mcap (replayer input), *.pcapng (netem), provider raw responses (live)
logs/                  stderr captures, never parsed
```

Verdicts are not part of a bundle: a verdict reads several bundles and lives under `runs/verdicts/` (HYP-20).

**TRC-23** `manifest.json` MUST be written last and MUST contain the `blake3` of every other file except those under `logs/`, which TRC-24 exempts from byte identity and which would otherwise make the manifest differ between identical runs. It MUST be UTF-8 JSON with sorted keys, no insignificant whitespace, numbers in the text form of CON-27(c) and one trailing newline. `bundle_digest` is the `blake3` of `manifest.json`; it names the data, where `run_id` names the experiment, and everything that cites a bundle (a verdict, an evidence page, a gate record) MUST cite the pair. `acn bundle verify` MUST recompute `run_id` (CON-29) and the hash of every listed file, MUST fail on any mismatch or on a file that is neither listed nor under `logs/`, and MUST print the `bundle_digest`.

**TRC-24** In `sim` mode, every file except `logs/` MUST be byte-identical across two runs with the same inputs (CON-5c); the acceptance test `tests/accept/trace_determinism.rs` MUST assert this on a fixture scenario.

## 6. Parquet layout (OTLP-shaped)

**TRC-25** `spans.parquet` columns: `trace_id` (FixedSizeBinary(16)), `span_id` (FixedSizeBinary(8)), `parent_span_id` (FixedSizeBinary(8), nullable), `name` (Utf8, dictionary), `kind` (Int8), `start_ns` (Int64), `end_ns` (Int64), `status_code` (Int8), `status_message` (Utf8, nullable), `resource_id` (Int32), `attrs` (Map<Utf8, Union{Utf8, Int64, Float64, Bool, Binary}>), plus **promoted columns** for every `acn.*` attribute marked `promoted = true` in `acn_attributes.toml` (typed, nullable), so analysis never parses the map. `events.parquet` and `links.parquet` mirror OTLP with `span_id` foreign keys. Row order MUST be `(start_ns, trace_id, span_id)` ascending. Parquet writer settings (compression `zstd` level 3, row-group size 65 536, statistics on, no wall-clock metadata) MUST be fixed in the schema module.

**TRC-26** Timestamps: `start_ns`/`end_ns` are nanoseconds on the run's `Clock` (CON-5b). In `sim` they are virtual and start at 0; in `live`/`netem` they are monotonic from run start, and `manifest.started_at` carries the only wall-clock time. Producers on other machines (external nodes) MUST have their clocks offset-corrected by the ingester using the proxy's request/response timestamps; the applied offset MUST be recorded as `acn.ingest.clock_offset_ns` on the affected spans.

**TRC-27** Trace and span IDs in `sim` mode MUST be generated by a seeded `IdGenerator` (the `opentelemetry_sdk::trace::IdGenerator` trait) drawing from the sub-stream `"trace.ids"` of the replicate the session belongs to, and of the run for the `acn.scenario` root span (CON-30b); in `live` mode the SDK default is permitted. This is an explicit clause of CON-5.

**TRC-28** An OTLP export path MUST exist: `acn bundle export --otlp <endpoint>` replays a bundle to any OTLP collector, and `acn bundle import --otlp-json <file>` ingests OTLP JSON, so external viewers and external producers interoperate without bespoke code.

## 7. Derived views (the report's §3.5 tables)

**TRC-30** The ingester (`acn_trace::ingest`) MUST compute the views from spans deterministically (fixed sort, no parallel reduction) and MUST be the only code that reads convention attributes (`gen_ai.*`, `http.*`, `network.*`); everything downstream reads views or `acn.*` promoted columns.

**TRC-31** `views/session.parquet` MUST hold one row per `acn.session` — identifiers from TRC-10, `turns`, `calls`, `duration_ns`, `input_tokens_total`, `cache_read_tokens_total`, `wire_bytes_up_total`, `wire_bytes_down_total`, `outcome_counts` (map).

**TRC-32** `views/turn.parquet` MUST hold one row per `acn.turn` — `session_id`, `turn_index`, `think_time_before_ns` (gap between the end of the previous turn of the session and the start of this one; null for the first turn), `chain_length` (calls in series), `fanout_width`, `duration_ns`, `first_useful_result_ns`, `outcome`, `network_wait_ns` (sum of link applied delay + rate-limit time on the turn's critical path), `tool_wait_ns`, `model_wait_ns`, `queue_wait_ns` (from TRC-18 when present), `stalls`, `retries`, `compaction`.

**TRC-33** `views/call.parquet` MUST hold one row per `chat` — session/turn/call indices, `lineage_id` (as in TRC-38), provider/model, `input_tokens`, `new_input_tokens`, `output_tokens`, `stop_reason`, `duration_ns`, and the preceding-tool columns `preceding_tool_count`, `preceding_tool_ns` and `preceding_tool_class` (over the `execute_tool` spans of the same context lineage and turn whose `acn.tool.requesting_call` is the index of the previous `chat` of that lineage, that is, the tools whose results this call consumed, identified causally and not by timestamps: their number; latest end minus earliest start, which is the wait this call experienced when tools ran in parallel; and the class of the longest member, ties broken by lowest `span_id`; all null when the set is empty), `cache_read_tokens`, `cache_write_tokens`, `cached_token_ratio`, `ttft_ns`, `itl_p50_ns`, `itl_p99_ns`, `wire_bytes_up`, `wire_bytes_down`, `server_prefill_ns`, `server_decode_ns`, `retries`, `error_class`.

**TRC-34** `views/link.parquet` MUST hold one row per `acn.link` — `call_id`, direction, bytes, applied delay, dropped, reordered, rate-limited time, scenario step in force, and `outage_id` when inside an outage.

**TRC-35** Views MUST be recomputable from the bundle's own `spans.parquet`, `events.parquet`, `links.parquet` and `resources.parquet` and from nothing else (stalls, scenario steps and outages are span events, and resource attributes live in their own table); `acn bundle verify --views` MUST recompute and compare them.

**TRC-36** Report coverage. Appendix A of this spec enumerates, with stable keys, every harness-side field of the report's §3.5 methodology and every parameter of its Appendix C sheet, for the report version named there. `docs/report/coverage.toml` (outside `crates/`: it is documentation-grade and MUST NOT move `env_hash` or `build_hash`) MUST map every key to exactly one of: `column` (a view and column), `attribute` (a promoted `acn.*` attribute), `derived` (an expression over named view columns, for example cached-token ratio or wire bytes per token), or `not_recorded` with the reason and the side-channel that holds the data instead (for example the decode packet-size and inter-arrival distribution, held as ITL quantiles here and in full only in a `pcapng` side-channel, TRC-41). `cargo xtask docs-inventory` MUST render the mapping as `docs/generated/report-coverage.md` and MUST fail on an Appendix A key with no entry, on an entry whose key is not in Appendix A, and on a named view or column that the view schema of TRC-37 does not define, or an attribute that `acn_attributes.toml` (TRC-20) does not list as promoted.

**TRC-37** View schema. `crates/acn-trace/src/schema/views.toml` MUST list every column of every view with its type, unit and nullability; the view writer MUST be generated from or checked against it, so the prose of TRC-31..34 and TRC-38 and the files on disk cannot drift. TRC-36 and the quantity table of HYP-12 resolve column names against this file.

**TRC-38** `views/tool.parquet` MUST hold one row per `execute_tool` — session and turn identifiers, `lineage_id` (the `span_id` of the enclosing `invoke_agent`, null on the session's main chain), `requesting_call` (the span's `acn.tool.requesting_call`, TRC-13; never inferred from timestamps), `tool_class`, `placement`, `duration_ns`, `result_bytes` — so that tool execution time by class and uplink bytes per tool result by class (Appendix A) have a column to map to.


## 8. Side-channels

**TRC-40** Sensor streams for `acn-replay` MUST be MCAP files under `sidecar/`, referenced from the manifest by hash; the replayer MUST emit `acn.replay.roundtrip` spans carrying `acn.replay.mcap_topic` and `acn.replay.log_time_ns` so a span can be joined back to its frame.

**TRC-41** Packet captures (`netem` mode) MAY be stored as `sidecar/*.pcapng`, referenced by hash, and MUST NOT be read on the run path or by views.

**TRC-42** Raw provider responses in `live` mode MAY be stored under `sidecar/provider/` for audit; they MUST be redacted of message content unless `acn.keep_content = true` is set on the run, and content MUST never be promoted into a view.

## 9. Acceptance tests (names are normative)

- `tests/accept/trace_determinism.rs` — TRC-24, TRC-27: two sim runs, byte-identical bundle.
- `tests/accept/trace_roundtrip.rs` — TRC-28: export to OTLP JSON and re-import yields identical views.
- `crates/acn-trace/tests/views.rs` — TRC-30..35 on a golden fixture with known chain length, fan-out, stalls and an outage.
- `crates/acn-trace/tests/attributes_inventory.rs` — TRC-20: every emitted `acn.*` attribute is listed; every promoted attribute has a typed column.
- `crates/acn-trace/tests/cache_mapping.rs` — TRC-21: per-provider raw fields map to `acn.cache.*`, to the total `input_tokens` and to the normalised `stop_reason` (an unknown value becomes `other`), and the source field name is recorded.
- `crates/acn-trace/tests/clock_offset.rs` — TRC-26: external spans are offset-corrected and annotated.
- `crates/acn-trace/tests/report_coverage.rs` — TRC-36: every Appendix C parameter and §3.5 field is mapped; an unmapped parameter and a mapping to a non-existent column both fail.
- `crates/acn-trace/tests/view_schema.rs` — TRC-37, TRC-38: every view on disk matches `views.toml`; the tool view's lineage and requesting call are derived from spans alone.

## 10. Open questions (ADR candidates)

1. Whether to promote `gen_ai.input.messages` (full prompt content) into the bundle at all; default no (TRC-42), revisit for POC 11 which needs prefix bytes — likely as hashes of prefix blocks, not content.
2. Whether `acn.link` spans per message are too fine-grained for long token streams; fallback is one span per stream with per-chunk events.
3. Adopt otel-arrow's OTAP encoding for `spans.parquet` if its Rust crates stabilise; the promoted-column design keeps that a storage-only change.

## Appendix A — report fields and parameters that TRC-36 tracks (report v1.2)

| Key | Report | What |
|---|---|---|
| `h.session_id` | §3.5 | session id |
| `h.turn_index` | §3.5 | turn index |
| `h.call_index` | §3.5 | call index within the turn and context lineage (TRC-12) |
| `h.input_tokens` | §3.5 | input tokens |
| `h.cached_input_tokens` | §3.5 | cached input tokens as returned by the API |
| `h.output_tokens` | §3.5 | output tokens |
| `h.stop_reason` | §3.5 | stop reason |
| `h.ttft` | §3.5 | wall-clock time to first token |
| `h.total_time` | §3.5 | wall-clock total time of the call |
| `h.preceding_tool_class` | §3.5 | tool class of the preceding tool call |
| `h.preceding_tool_time` | §3.5 | execution time of the preceding tool call |
| `h.spawn_parent_call` | §3.5 | sub-agent spawn events with the parent call id |
| `s.turns_per_session` | App. C | turns per session |
| `s.think_time` | App. C | think-time between turns |
| `s.session_lifetime` | App. C | session lifetime |
| `t.chain_length` | App. C | chain length *k* |
| `t.tool_exec_time_by_class` | App. C | tool execution time by class |
| `t.fanout_degree_depth` | App. C | fan-out degree and depth |
| `t.compaction_vs_length` | App. C | compaction probability against transcript length |
| `c.input_length_by_call_index` | App. C | input length by call index |
| `c.new_tokens_per_call` | App. C | new tokens per call |
| `c.output_length_by_call_type` | App. C | output length by call type (tool call or final answer, from `stop_reason`) |
| `c.cached_token_ratio` | App. C | cached-token ratio |
| `b.wire_bytes_per_token` | App. C | wire bytes per token |
| `b.decode_packet_size_interarrival` | App. C | decode packet size and inter-arrival |
| `b.uplink_bytes_per_tool_result` | App. C | uplink bytes per tool result by class |
| `f.diurnal_weekly` | App. C | diurnal and weekly pattern by time zone |
| `f.cross_session_prefix_overlap` | App. C | cross-session prefix overlap |
| `f.batch_burst_structure` | App. C | batch-job burst structure |

Fleet-level keys are properties of many sessions and are expected to be `derived` or `not_recorded`; listing them keeps the omission explicit.
