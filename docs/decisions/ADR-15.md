# ADR-15 — T02d: OTLP export and import, report coverage, clock offsets

**Status:** accepted (T02d, Class B). **IDs affected:** TRC-18, TRC-26, TRC-28, TRC-36.

## Context
T02d is the last step of ADR-11. TRC-28 names the commands but not what an import produces. TRC-26 requires offset correction "using the proxy's request/response timestamps" but not the estimator. TRC-36 names the mapping file but not its format.

## Decision
- **OTLP/JSON, not protobuf.** Export writes the JSON encoding of an `ExportTraceServiceRequest`, which every OTLP/HTTP collector accepts at `/v1/traces` with `Content-Type: application/json`. That keeps the codec small, readable and dependency-free. It follows the OTLP JSON rules: hex ids, 64-bit integers as strings, base64 bytes, integer enums.
- **Export and import are inverse.**
  - One `resourceSpans` per resource, with one scope (`acn-bench`).
  - Events and links keep their order, which carries `seq`.
  - Import numbers resources by their sorted attributes, as the collector does.

  A bundle exported and re-imported yields the same tables and byte-identical views (`tests/accept/trace_roundtrip.rs`).
- **Import is strict.** It refuses array and map values, dropped attributes, events or links, non-finite doubles, duplicate attributes, malformed ids and times. Fields the model does not hold (trace state, flags, schema URLs, scope versions) are ignored.
- **`acn bundle export <dir>`** verifies the bundle (TRC-23) and then:
  - `--otlp <endpoint>` POSTs the document and requires a 2xx;
  - `--otlp-json <file>` writes it, never replacing a file.

  The HTTP client is `reqwest` with rustls and the operating system's trust store (`rustls-tls-native-roots`). The bundled `webpki-roots` list is CDLA-Permissive-2.0, which the licence policy does not allow, and the system store is the better default for a CLI anyway.
- **`acn bundle import --otlp-json <file> --out <dir>`** decodes, aligns foreign clocks and writes the four tables and five views into a new directory. That directory is not a run bundle: it has no manifest and no `run_id`, because the document carries no run identity. The run path (T04, T30) will add external producers' spans to a live run with the same calls (`otlp::from_json`, `Trace::merge`, `ingest::align_clocks`) before `Bundle::finish`; nothing does that yet.
- **Clock offsets (TRC-26).**
  - **Which spans.** A span whose resource lacks `acn.engine_hash` comes from a producer on another machine (TRC-19 makes every acn-bench producer record it). An external span parented to a `chat` roots a subtree on one foreign clock.
  - **Estimator.** Assume equal delay each way, as NTP does, and centre the foreign interval on the reference: `offset = ((start − arrival) + (end − departure)) / 2`, rounded toward negative infinity.
  - **Reference.** The call's `acn.link` spans when the proxy recorded both directions: the last uplink dequeue is when the request arrived, and the last downlink enqueue is when the response left. Otherwise the call's own start and end.
  - **Applying it.** Every span of the subtree and its events shift by the offset, and each span records it as `acn.ingest.clock_offset_ns`. Spans that already carry it are left alone, so the step is idempotent.
  - **Refusal.** An external span under no call is an error: nothing can align it.
- **TRC-18 is not implemented.** Attaching node spans to their call is the span tree itself. Deriving `acn.server.*` from them needs a per-node mapping, which belongs in the frozen inventory the way the provider mappings are (TRC-21). That is a Class C change for when a vLLM or SGLang node exists (T30).
- **Report coverage (TRC-36).** `docs/report/coverage.toml` holds one `[[key]]` per Appendix A key:
  - `kind = "column"` with a view and a column;
  - `kind = "attribute"` with a promoted attribute;
  - `kind = "derived"` with an expression and every `view.column` it reads;
  - `kind = "not_recorded"` with a reason and a side channel.

  `acn_trace::coverage` parses it and is shared by `docs-inventory` and the named test. It reads the keys from SPEC 010 itself, and requires each kind to carry exactly its fields. It renders `docs/generated/report-coverage.md` in Appendix A order. Four keys are recorded as absent:
  - the decode packet distribution (ITL quantiles only; pcapng in netem);
  - diurnal and weekly patterns (a fleet property; a run has no wall clock);
  - cross-session prefix overlap (content is never recorded; prefix-block hashes are planned for POC 11);
  - batch burst structure (session starts are span timestamps, not a view column).

## Consequences
TRC-26, TRC-28 and TRC-36 enter scope, and with them SPEC 010 is implemented except TRC-1, TRC-18 (above), TRC-19's commit-identifier clause, TRC-40 to TRC-42 (side channels, T50 and later) and the value-set checks of TRC-3 and TRC-12. The T02 series of ADR-11 is complete.

## Amendment (pre-landing review of PR 4)
Three separate sessions reviewed this PR, one cross-review with an adversarial brief. Findings fixed here:
- **Export cannot hang or mislead.**
  - Every request has a connect and an overall deadline (`--timeout-secs`, default 30).
  - Redirects are refused: a redirected POST becomes a body-less GET that a 200 would have reported as delivered.
  - Anything but a 2xx is an error.
  - The client is built with `Client::builder()…build()?`, so a trust-store failure is an error, not a panic.
  - Credentials in the endpoint are stripped from the output.
  - Proxy variables (`HTTP_PROXY` and so on) are honoured on purpose. Export is not a run (CON-29), and a collector behind a corporate proxy must stay reachable.
- **Export is linear and chunked.** Events and links are grouped by span once (the first version scanned them per span: 7 s at 32k spans). `--max-spans` (default 2000) splits the POSTs, so no request outgrows a collector's body limit. The split is a function of the trace alone. `--otlp-json` writes one document, through a temporary file linked into place, so it never leaves a partial file and never replaces one. Export refuses a span naming no resource, and an event or link naming no span, instead of dropping them.
- **Import reads proto3 JSON as writers emit it.** A field at its default is omitted, and import reads it as 0, `""` or enum 0, as protobuf JSON parsers do; a sim session starting at 0 loses its start time in a collector round trip. Enum names (`SPAN_KIND_SERVER`, `STATUS_CODE_ERROR`) are accepted, and `kind` and `status.code` are range-checked. A double may be a numeric string. A hand-written SDK-shaped fixture is decoded and checked field by field.
- **Import writes all or nothing.** Tables and views are encoded and checked first (`bundle::encode_tables_and_views`, shared with `Bundle::finish`). They are written into a temporary sibling directory and renamed into place, so a failure leaves nothing and a retry is not refused.
- **A node's own export can be merged.** `acn bundle import --otlp-json <node.json> --with <bundle> --out <dir>` verifies the bundle and merges its trace with the node's spans. `Trace::merge` renumbers resources with the same rule as the collector (`Trace::renumber_resources`). The merged trace is then aligned and written. Without `--with`, a node's export names parents it does not hold, and import refuses it.
- **Clock alignment.**
  - A link direction other than `up` or `down` is an error, not a downlink.
  - All roots of one producer under one call share one offset: they share one machine clock (a retried request). The offset centres their envelope, the earliest root start to the latest root end.
- **Report coverage.** A `derived` entry's expression may name only columns its `columns` list holds, and the loader checks it. `t.compaction_vs_length` now lists its join columns. `docs-inventory` itself is tested to fail on a missing or wrong mapping.

Decided in review, recorded here:
- **Arrays stay refused.** OTLP array attributes (such as GenAI's `gen_ai.response.finish_reasons`) are refused, not flattened; the profile stores scalars (TRC-25).
- **Duplication left for later.** Hex encoding is still written in several modules, and alignment still lives in `ingest`. Consolidating them is left for a refactor that changes no behaviour.
