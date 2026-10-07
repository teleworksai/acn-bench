# ADR-40 — T15: attribution, its network time and where its quantities live

**Status:** accepted (T15.1, spec-change). **IDs affected:** ATR-1 to ATR-42, HYP-12, TRC-32.

## Context
T15 builds SPEC 090. Three questions had to be settled first:
- **ADR-14 left one for the maintainer.** TRC-32's `network_wait_ns` is a sum of per-segment delays. A streamed call with overlapping segments can report more network wait than its duration, and a negative `model_wait_ns`. ADR-14 asked for this to be settled before SPEC 090 builds on it.
- **HYP-12 conflicts with PLAN.md.** HYP-12 puts every quantity in `acn-hyp`'s table, and attribution quantities "defined by SPEC 090 and registered here by name". But PLAN.md let `acn-hyp` and `acn-attrib` depend on `acn-trace` only, so the verdict could not call attribution code.
- **The task is large, and part of it is in the frozen set.** It spans a spec, frozen code (`acn-attrib/src/core/`, CON-7) and ordinary code (plots, the CLI).

## Decision
The maintainer chose each of the following (October 2026).
- **Attribution's network time is elapsed time,** computed by SPEC 090 itself from the link view (ATR-11). TRC-32's column stays as it is (changing it would be a Class C schema change), and attribution reads it only to check itself against the views (ATR-14).
- **ADR-14's readings are confirmed** as attribution's basis: the last-finishing-predecessor walk, the leaves and the tie rules. `acn-trace` exposes the walk, with each turn's start and end (ATR-2), so the turn view and attribution share one implementation.
- **`acn-hyp` depends on `acn-attrib`** (ATR-22). There is one implementation of each attribution quantity, and `acn-hyp`'s table registers it by name. The comparison with the control and its interval stay the verdict's (HYP-13, HYP-15), and attribution adds no second bootstrap (ATR-23). This is how TASKS.md's "CI via bootstrap" is met.
- **Three PRs** (ADR-11):
  - T15.1, this one: SPEC 090.
  - T15.2: the core, the critical path from `acn-trace`, and `acn-hyp`'s registration. It is a Class C change, labelled `env-change`, with an adversarial review, and merged by the maintainer.
  - T15.3: `acn attrib turns` and `acn attrib heatmap`, which are Class B.
- **Hops are links in one direction.** Each network interval belongs to the hop of its link row (ATR-13).
- **The tail is nearest-rank.** The tail turns are those at or above the nearest-rank 99th percentile of turn duration, ties included (ATR-21). This is the same rank rule as `ttft_p99_ms`.

## Consequences
- T15.2 moves `engine_hash`, and with it every `run_id`. A bundle made before it no longer matches a current binary's engine, as for any frozen change after M0.
- No ATR id is implemented by this PR. `trace-scope.toml` gains them as T15.2 and T15.3 land.

## After review (two read-only reviews of the draft)
The first draft took a leaf's network time as the union of its link intervals. That was wrong in three ways the reviews showed, and §2 was rewritten.
- **Streaming.** A streamed answer is one downlink message per event (EMU-32). When events are closer together than the one-way delay, their intervals cover the whole decode, so the share approached 1 whatever the delay. Now a call is read attempt by attempt (ATR-11):
  - the request's interval is network time;
  - the time from the request's receipt to the last message's send is the server's (model) time, during which earlier messages travel alongside;
  - the last message's interval is network time.

  A link of one-way delay `d` gives exactly `2d`, streamed or not. The acceptance suite calibrates that against a no-delay control.
- **Loss and retries.** A dropped message has an empty interval, and the backoff is on no wire, so loss showed up as *model* time and could lower the share. Now:
  - a lost request's wait, and a lost last message's wait, are network time;
  - failed attempts and their backoff are a fifth cause, `retry_ns`.

  Charging a retry to its cause needs per-attempt failure classes in the trace (open question 2).
- **Attempts from times alone.** Each attempt's request is one uplink row, and an attempt carries only the messages it received before it ended (`net.rs`, `exchange`). So a downlink row belongs to the last attempt sent at or before it. This needs no schema change.
- **The check against the views.** ATR-14 compares tool time with `tool_wait_ns`, and chat time with `model_wait_ns` plus the chat leaves' summed delay and rate-limit time, which is exactly what the turn view subtracted. The earlier form wrongly included remote tools' link time.
- **Scope.** "Network" is the emulated network (§ Scope, ATR-42). A real provider's WAN outside the emulator counts as model time.
- **The frozen boundary.** Cargo cannot depend on half a crate, so `acn-attrib`'s plots sit behind a `plots` feature that `acn-hyp` does not enable (ATR-22). The heatmap reads `verdict.json` as JSON. Plots stay in `acn-attrib`, as CLAUDE.md says. The walk lives in non-frozen `acn-trace`. A verdict's byte-identical view recomputation (TRC-35) and ATR-14 keep it honest, which ATR-22 now says rather than claiming the walk is frozen.
- **Edge cases are rules, not inferences:** clipping and `clipped_ns`, rows with no call (`unattributed_link_ns`), the hop order, the integer rank, the per-(role, replicate) output, the heatmap's slice and parameters, CON-27(c) numbers, and a bundle-wide error that a verdict turns into a refusal (ATR-15).
