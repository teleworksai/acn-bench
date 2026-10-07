# SPEC 090 — Attribution

**Status:** Draft v0.1 (October 2026; written for T15). **Inherits:** SPEC 000, 010, 020, 040, 080. **Prefix:** ATR. **Crates:** `acn-attrib` (its frozen core, `src/core/`, CON-7, and its plots), `acn-trace` (the critical path it exposes), `acn-hyp` (the quantities it registers), `acn-cli` (`acn attrib`).
**Purpose:**
- say where a turn's time went: how much the emulated network took, on which hop, and how much the model, the tools, retries and everything else took;
- define the attribution quantities a hypothesis names, above all `network_attributable_share`, once, in frozen code, so that a verdict and a report compute the same number;
- turn a verdict's cells into a heatmap a person can read.

Attribution decides nothing about a hypothesis. The treatment-minus-control effect of an attribution quantity, its confidence interval and its noise floor are the verdict's (HYP-13, HYP-15); this spec defines the quantity.

**Scope of "network".** Network time is time on the links of the run's scenario (SPEC 020), as the link view records it. A run without a scenario, or a real provider's WAN, TLS and edge outside the emulator, has no link rows: that time is counted as model time. A number from this spec is the share of the turn's critical path spent on the *emulated* network, and an evidence page that cites it MUST say so (ATR-42).

## 0. Terms

- **Turn** — a row of the turn view (TRC-32), with its start and end (ATR-2).
- **Critical path** and its **leaves** — the chats and tools a turn waited on, found by the walk of ADR-14 (ATR-2). A leaf's time is its end less its start.
- **Link row** — a row of the link view (TRC-34). Its **interval** runs from `enqueue_ns` (sent) to `dequeue_ns` (received): applied delay, rate-limit and serialization time, and the head-of-line wait for an earlier message, are all inside it. A dropped message has `dropped = true`; its interval is empty. Both times are on the run clock, as span times are.
- **Attempt** — one request of a call and the answer it got. Its request is one uplink link row (EMU-32, EMU-36); its answer is the downlink rows it carried before it ended.
- **Hop** — one link in one direction: a (`link_id`, `direction`) pair, ordered bytewise by `link_id` (UTF-8), then by `direction`.
- **Cause** — one of `network`, `model`, `tool`, `retry` and `other` (ATR-10).

## 1. Inputs

**ATR-1** Attribution MUST read a bundle only through `acn-trace`: its views (TRC-30) and the critical path of ATR-2, from a bundle that verifies with its views (TRC-23, TRC-35). It MUST NOT read a convention attribute (TRC-3), and it MUST NOT read a span's timestamps except as ATR-2 hands them over.

**ATR-2** `acn-trace` MUST expose, for every turn, its start and end and the leaves of its critical path as the turn view computes them (ADR-14): each leaf's span id, kind (`chat` or `execute_tool`), start and end, and, for a tool, its `placement`. There MUST be one implementation of the walk, used by both the turn view and attribution.

## 2. A turn's decomposition

**ATR-10** For every turn, attribution MUST split its `duration_ns` into five integer parts, in nanoseconds, that are never negative and sum to it exactly: `network_ns`, `model_ns`, `tool_ns` and `retry_ns`, each the sum of the leaves' parts of ATR-11 to ATR-13, and `other_ns`, the rest.
- The leaves of one critical path do not overlap when every span lies inside its parent: the walk steps back only to a span that ends at or before the current one starts. Leaves that overlap, or reach outside the turn, MUST be an error naming the turn, never a clamped number.
- A turn with no leaves is all `other_ns`. A zero-length turn or leaf has zero parts.
- Time a sub-agent or a fan-out tool spends outside the leaves that replace it on the path (ADR-14) is `other_ns`.

**ATR-11** A chat leaf's parts. Its link rows are those whose `call_id` is the leaf, clipped to the leaf's start and end; the nanoseconds removed by clipping are recorded (ATR-14). With no link rows, the leaf's whole time is `model_ns`. Otherwise:
- The attempts are the uplink rows in `enqueue_ns` order. A downlink row belongs to the last attempt whose uplink row was sent at or before it was. A downlink row sent before the first uplink row, or before its own attempt's request was received, is an error.
- `retry_ns` is the time from the first attempt's send to the last attempt's send: the failed attempts and the backoff between them.
- The time before the first attempt's send is `other_ns` (client work).
- The last attempt, from its send to the leaf's end:
  - if its request was dropped, all of it is `network_ns`: the call waited on a lost message;
  - otherwise the request's interval is `network_ns`. If it got no downlink row, the rest is `model_ns`: the call waited on the server. If it got one, let `r` be its last downlink row, the latest sent, ties by row order. From the request's receipt to `r`'s send is `model_ns`: the server's time, during which a streamed answer's earlier messages travel alongside it. If `r` was dropped, from its send to the leaf's end is `network_ns`. Otherwise `r`'s interval is `network_ns`, and from its receipt to the leaf's end is `other_ns`.

A streamed answer over a link of constant one-way delay `d` therefore gives `2d` of network time, and a request lost on the way is network time, not model time.

**ATR-12** A tool leaf's parts. A tool whose `placement` is not `remote` is all `tool_ns`. A remote tool is split as a chat is, with its server's time as `tool_ns` instead of `model_ns`.

**ATR-13** Each leaf's `network_ns` MUST also be split by hop: each of its network intervals of ATR-11 belongs to the hop of the link row it came from, and a lost request's wait to the hop of that request. A turn's hop parts sum to its `network_ns` exactly.

**ATR-14** Every turn MUST also carry:
- its diagnostic counts from the turn view: `queue_wait_ns` (nullable), `stalls` and `retries`. They are not causes: server queueing is part of `model_ns`;
- `clipped_ns`, the link time removed by clipping, and `unattributed_link_ns`, the time of link rows with no `call_id` whose interval overlaps the turn.

And it MUST be checked against the turn view: the tool leaves' summed time MUST equal `tool_wait_ns`, and the chat leaves' summed time MUST equal `model_wait_ns` plus the chat leaves' summed `applied_delay_ns` and `rate_limited_ns` (ADR-14's reading of TRC-32). A turn that fails is an error: the views and attribution read different paths. Attribution reads `network_wait_ns` for nothing else.

**ATR-15** An error in a turn MUST make the whole bundle's attribution an error, naming the turn. A verdict that needs an attribution quantity of that bundle refuses (`ok: false`), as for any bundle it cannot read (HYP-20); it never treats the value as undefined.

## 3. Quantities

**ATR-20** `network_attributable_share` of a replicate MUST be the sum of `network_ns` over its turns divided by the sum of their `duration_ns`: two integer sums and one division, `(num as f64) / (den as f64)`. It is undefined when the replicate has no turn or its turns' durations sum to zero. `retry_share`, `model_share`, `tool_share` and `other_share` are defined the same way.

**ATR-21** Tail decomposition. For a replicate of `n` turns and a percentile `p`, the **tail turns** MUST be the turns whose `duration_ns` is at or above the nearest-rank `p`-th percentile of the replicate's turn durations, rank `max(1, ceil(n·p/100))` computed in integers as `(n·p).div_ceil(100)`, ties included. `tail_<cause>_share_p99` MUST be the sum of that cause's part over the tail turns divided by the sum of their durations, for each cause. They are undefined as ATR-20 is. With fewer than 100 turns the tail is the longest turn and its ties.

**ATR-22** The quantities of ATR-20 and ATR-21 MUST be implemented in `acn-attrib` and registered in `acn-hyp`'s quantity table (HYP-12) by name, unit and formula. `acn-hyp` calls `acn-attrib`, so a verdict and a report compute one number.
- `acn-attrib`'s core depends on `acn-trace` alone. Its plots (ATR-41) live outside `src/core/`, behind a `plots` feature that brings in `plotters`; `acn-hyp` depends on `acn-attrib` without it, so nothing outside the frozen core is compiled into the verdict engine. The heatmap reads `verdict.json` as JSON, not through `acn-hyp`'s types.
- `acn-hyp` depends on `acn-attrib`; `acn-attrib` never depends on `acn-hyp`.
- The walk of ATR-2 lives in `acn-trace`, outside the frozen set. What keeps it honest is that a verdict recomputes the views from the bundle's tables and requires them byte-identical (TRC-35), and ATR-14 checks attribution against them.

**ATR-23** A comparison with the control MUST be the verdict's `effect`, `ci_low` and `ci_high` of the quantity (HYP-13), on the paired bootstrap of HYP-15. Attribution MUST NOT compute a second effect or a second confidence interval of its own.

## 4. Determinism

**ATR-30** Attribution MUST compute in integers until the last step:
- every time is an `i64` of nanoseconds, every sum an exact integer sum, and an overflow an error;
- a quantity's one division is the last operation, in `f64`;
- the order of every reduction is fixed (turns in view order, leaves in path order, link rows in view order, hops in hop order), and there is no parallel reduction.

So no number depends on evaluation order or platform.

**ATR-31** A change to `acn-attrib`'s core is a change to the frozen set (CON-7): it moves `engine_hash` (CON-28) and is a Class C change. Code outside the core, the plots included, MUST NOT compute an attribution quantity; it calls the core.

## 5. Outputs

**ATR-40** `acn attrib turns <bundle> --out <dir>` MUST write `<dir>/attribution.parquet` and print one JSON object (CON-8).
- The file has one row per turn, in view order, with columns `run_id`, `role`, `replicate`, `session_id`, `turn_index`, `duration_ns`, the five parts, the counts and diagnostics of ATR-14, and `hop_ns`, a map from `<link_id>/<direction>` to nanoseconds in hop order. The schema is fixed by a test and the same bundle MUST give a byte-identical file. Nothing is written into the bundle, which is immutable (TRC-23).
- The object carries `ok`, the bundle's `run_id`, its `mode` and `backend`, the file's BLAKE3 and row count, and `replicates`: per (`role`, `replicate`), in that order, the ATR-20 and ATR-21 values, a number written as CON-27(c) says or `null` when undefined. A `mockllm` or `sim` bundle carries the labels of CON-26 and HYP-23, as a verdict would, so that the output never reads as citable.

**ATR-41** `acn attrib heatmap --verdict <verdict.json> --slice <key> --quantity <q> --x <param> --y <param> --out <file.svg>` MUST draw, with `plotters`, one cell per pair of values of two varied parameters in one slice of the verdict, coloured by the verdict's `effect` of `q` in that cell, with the value and the recorded interval written in the cell as CON-27(c) says.
- `q` MUST be one of the verdict's measured quantities, and the slice MUST vary no parameter other than `x` and `y`; otherwise it is an error.
- A cell without a value is drawn empty and marked.
- The title MUST carry the verdict's labels (HYP-23), so that an exploratory, mock-gated, unpinned or sim-only result never looks citable.
- The same verdict and arguments MUST give a byte-identical SVG.

**ATR-42** An evidence page (LOOP-30) that shows an attribution quantity MUST state that its network time is the emulated network's (§ Scope).

## 6. Acceptance tests

- `crates/acn-attrib/tests/decompose.rs` — ATR-1, ATR-10 to ATR-15, ATR-30. Hand-built traces with known answers for:
  - a plain call over delay `d` (`2d` of network) and the same call streamed (still `2d`);
  - a lost request and its retry, a cut stream and its retry, a timeout, and a backoff (`retry_ns`);
  - a lost last downlink message; an answer-less attempt;
  - a link row reaching past its chat (clipped, recorded), a link row with no `call_id`, and a downlink row before any uplink (an error);
  - a remote tool, a local tool, nested sub-agent leaves, a turn with no leaves and a zero-length turn;
  - two hops and the hop order.
  
  It also checks that the parts sum to the duration and pass ATR-14 on every turn of the fixtures and of a sim bundle, and that overlapping or escaping leaves fail the bundle.
- `crates/acn-attrib/tests/quantities.rs` — ATR-20 to ATR-22, ATR-30: known answers for every share and tail share, ties at the tail's threshold, `n` = 1, 10 and 100, and the undefined cases; `acn-hyp`'s table resolves each name to `acn-attrib`.
- `crates/acn-trace/tests/` — ATR-2: the leaves attribution reads are the ones the turn view used.
- `crates/acn-cli/tests/attrib_cli.rs` — ATR-40, ATR-41, CON-8: one JSON object; byte-identical Parquet and SVG on reruns; the labels; an empty cell marked; the errors of ATR-41.
- `tests/accept/attrib.rs` — ATR-11, ATR-20, ATR-23, CON-18. A calibration on the mock in `sim`:
  - a non-streamed workload with no tools over a scenario of one-way delay `d` against the same workload with no delay, the control: each call's `network_ns` is exactly `2d`, the treatment's share is its known value, and the verdict's `effect` is the known difference, inside its bootstrap interval;
  - the same, streamed.

## 7. Open questions

1. **Off the critical path.** Network time on a sub-agent that runs beside the critical path counts for nothing, and a delay that makes another sibling critical switches the path. The share is the critical path's, not the total network cost, and it can overstate what removing a delay would save.
2. **Retries by cause.** `retry_ns` holds every failed attempt and its backoff, whatever failed. Charging a retry caused by loss to the network and one caused by a 5xx to the model needs each attempt's failure class in the trace, a SPEC 010 and SPEC 040 change.
3. **Several hops per direction.** A scenario path has one link per direction today. A path of several links would split a message's interval among them, which the link view does not yet record.
4. **Remote tools.** The harness runs every tool locally (`placement = "local"`), so ATR-12's remote case is exercised only by fixtures until a remote tool exists.
