# ADR-14 — T02c: the ingester, the critical path and the views

**Status:** accepted (T02c, Class B). **IDs affected:** TRC-10, TRC-22, TRC-30 to TRC-35, TRC-37, TRC-38.

## Context
T02c is the fourth step of ADR-11: `acn_trace::ingest` and the five derived views. TRC-32 defines the turn's waits "on the turn's critical path" and `chain_length` as "calls in series" without saying how either is found. Every network-attributable number of POC 1a and 1b will rest on these readings, so they are recorded here, and the maintainer should confirm them before SPEC 090 (ATR) builds on them.

## Decision
- **Critical path.** Take the work spans directly under a container (a turn, or a sub-agent): `chat`, `execute_tool`, `invoke_agent`. Start from the one that ends last. Step back to the one that ends latest at or before the current one's start, and repeat. Ties are settled as the amendment below says. This is the usual last-finishing-predecessor walk, and it uses only timestamps and parentage, so it is deterministic. A path member that has work spans of its own (a sub-agent, or a tool that spawned one) is replaced by its own critical path. The leaves are the chats and tools the turn waited on.
- **The turn's columns from that path:**
  - `chain_length`: the main-chain chats among the leaves (their parent is the turn).
  - `tool_wait_ns`: the summed durations of the tool leaves.
  - `network_wait_ns`: the sum of applied delay plus rate-limited time over the `acn.link` spans of the chat leaves and of the remote tool leaves, literally as TRC-32 says. Link segments of one call overlap in time, so this is a sum of per-segment delays, not elapsed waiting. SPEC 090 may refine it, and the column says what it is.
  - `model_wait_ns`: chat-leaf time less the chat leaves' link wait. Because the network term is a sum of overlapping delays, it can be negative. It is reported as computed, never clamped.
  - `queue_wait_ns`: the sum of the chat leaves' `acn.server.queue_ms`, null if any of them lacks it.
- **Counts over the whole turn,** path or not:
  - `stalls`: the `acn.stream.stall` events of every chat in the turn.
  - `retries`: the sum of `acn.call.retries` over those chats.
  - `fanout_width` and `fanout_depth`: the largest values over every `invoke_agent` in the turn.
- **Think time** is measured between turns in `acn.turn.index` order, and two turns of a session with one index are an error.
- **Lineage** is the nearest `invoke_agent` between a call and its turn; `null` means the main chain. The previous chat of TRC-33 is the one with `acn.call.index` one less in the same lineage and turn. Its tools are those of that lineage and turn whose `acn.tool.requesting_call` names it, so a predecessor in an earlier turn gives an empty set.
- **`ttft_ns`:**
  - Streamed call: the first `acn.stream.first_token` minus start.
  - Non-streamed call: the call's duration, when `acn.call.ttft_ms` is present. That attribute is present exactly when a token arrived (TRC-12).
  - Otherwise: null.
- **The link view.**
  - `call_id` is the link span's parent when that is a chat or a remote tool; it is null for a link span with no parent. Any other parent is an error.
  - `run_id` is the one `acn.run_id` every session of the trace carries (TRC-35: from the spans alone). Sessions that disagree, or link spans without a session, are errors.
  - Scenario steps and outages are the events of the one `acn.scenario` span.
- **Strictness.** A required attribute that is missing, `gen_ai.provider.name` and `gen_ai.request.model` included, is an error naming the span, never a null in a non-nullable column. A sum that overflows is an error. Rows follow the stored order of their spans, except turn rows, which are ordered by session, then by `acn.turn.index`.
- **Checked against `views.toml` (TRC-37).** The Arrow schema of each view is generated from `views.toml`. Each row must produce every declared column with its type, and no other column. A null in a non-nullable column is an error.
- **Bundles hold their views (TRC-22).** `Bundle::finish` computes the views before it writes anything and requires every session to name the bundle's run (TRC-10). `verify` requires all five view files. `verify_views` (`acn bundle verify --views`, TRC-35) reads the four tables back, recomputes the views from them alone, and requires each file to be byte-identical to the recomputation, which the deterministic writer makes a meaningful comparison.
- **Only the ingester reads convention attributes (TRC-30).** A test scans every crate's sources for `gen_ai.`, `http.` and `network.` literals. Producers (the fixture) write them, and the schema names two only to forbid them as view sources (TRC-42).
- **The map columns** use Arrow's `key`/`value` field names, like `attrs`.

## Consequences
TRC-22 and TRC-30 to TRC-35, TRC-37 and TRC-38 enter scope. The views of the determinism acceptance suite join its byte-identity comparison. `server_prefill_ns` and `server_decode_ns` read `acn.server.*` today; their derivation from external inference-node spans (TRC-18) and the clock-offset correction land with T02d.

## Amendment (pre-landing review of PR 3)
Three separate sessions reviewed this PR, one cross-review with an adversarial brief. Findings fixed here:
- **Spans on the path are never candidates again.** The first walk stopped when a zero-length span turned out to be its own predecessor, dropping every earlier member. Which spans it dropped depended on the random span ids. A span already on the path is now excluded and the walk continues.
- **End-time ties are settled by time and kind, not by id.** Among spans ending at the same instant:
  1. The one that started earliest wins, because the turn waited on the longer span.
  2. Then a chat wins over a tool, and a tool over a sub-agent.
  3. Only spans identical in time and kind fall back to the lowest span id.

  Seeded ids therefore no longer move time between the tool, model and network columns.
- **The ingester cannot hang or overflow.** A parent cycle, or nesting deeper than `MAX_DEPTH` (256), is an error. The depth is computed iteratively, which bounds the critical-path recursion.
- **Call indices are checked (TRC-12, TRC-13).** Within a turn and lineage, `acn.call.index` must be 0..n in start order, and every `acn.tool.requesting_call` must name one of those calls. Both fixtures had broken this: they numbered calls across turns. They now restart at 0 each turn.
- **Inputs that would give silently wrong numbers are errors:**
  - a span that ends before it starts;
  - a negative `_ms` value;
  - an outage that ends before it starts;
  - an event on no span, or a stream event not on a chat or a scenario event not on the scenario;
  - link spans without exactly one `acn.scenario` span (TRC-16);
  - an inventory without the outcome value set.

  Counts and sums never saturate; on overflow they are errors.
- **Step ties follow emission order.** Steps at one instant are ordered by `seq`, so the step emitted last is in force, never the one whose name sorts last.
- **`verify --views` checks the tables too.** It re-encodes the four tables from the rows it read and requires byte equality. A promoted column that disagrees with `attrs` was invisible to the views, which read `attrs`; it is now caught. Writing and verifying use one encoder.
- **View paths come from `views.toml` only.** The writer, the layout check and `verify` all read the view files from it.

Still open, for the maintainer: whether `network_wait_ns` should stay TRC-32's literal sum of per-segment delays, or become elapsed time, for example the union of each call's delay intervals. A streamed call with many delayed segments can report more network wait than its duration, and a negative `model_wait_ns`. This must be settled before SPEC 090 builds on it.
