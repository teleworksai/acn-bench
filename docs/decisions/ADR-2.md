# ADR-2 — OpenTelemetry as the event model; acn-bench owns the Parquet writer

**Status:** accepted (draft v0.1). **IDs affected:** CON-4, CON-5, TRC-1..4, TRC-20, TRC-25, TRC-27.

## Context
acn-bench needs a trace format that external tools can read, that inference engines and agent tooling already emit, and that stays byte-deterministic in `sim` mode. The GenAI semantic conventions are still in Development status (no Stable attributes as of July 2026) and have renamed attributes several times; the upstream otel-arrow Parquet exporter was closed as not planned.

## Decision
Adopt OpenTelemetry spans/events/resources as the event model with W3C context propagation; pin the convention version; keep every verdict-relevant quantity under an `acn.*` namespace mapped at ingest; store as OTLP-shaped Parquet written by `acn-trace` with promoted typed columns; seed the OTel `IdGenerator` in `sim` mode; MCAP and pcapng as side-channels only.

## Consequences
Convention renames never change a verdict (only the ingest mapping). External viewers work on any bundle via OTLP export. acn-bench maintains a small Parquet writer and an attribute inventory; switching to OTAP encoding later is a storage-only change.
