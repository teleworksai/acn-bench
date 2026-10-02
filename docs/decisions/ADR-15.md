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
- **`acn bundle import --otlp-json <file> --out <dir>`** decodes, aligns foreign clocks and writes the four tables and five views into a new directory. That directory is not a run bundle: it has no manifest and no `run_id`, because the document carries no run identity. Adding external producers' spans to a live run is the run path's job (T04, T30). It calls `otlp::from_json` and `ingest::align_clocks` on the merged trace before `Bundle::finish`.
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
