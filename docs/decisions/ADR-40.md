# ADR-40 — T15: attribution, its network time and where its quantities live

**Status:** accepted (T15.1, spec-change; T15.2, env-change and spec-change). **IDs affected:** ATR-1 to ATR-42, HYP-12, TRC-32, CON-7, CON-28.

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

## T15.2 notes
- **The walk is shared.** `acn_trace::ingest::critical_paths` returns each turn's start, end and leaves, in turn-view order, from the same `critical_path` and the same turn ordering (`turn_order`) as the turn view. `bundle::verify_views_read_trace` returns the verified trace beside the views, so a verdict reads the paths from what it verified. The walk's doc comment now states ADR-14's amended tie rule.
- **The core takes rows, not files.** `acn_attrib::core::decompose` takes the turn view's rows, the paths and the link view's rows, as typed structs. `acn-hyp`'s `read` decodes them from the verified views (`decompose_views`), so `acn-attrib`'s only workspace dependency is `acn-trace` (with `thiserror`), and it has no direct Arrow dependency; Arrow comes in through `acn-trace`'s `io` feature, which the ingester needs (ATR-22).
- **Which rows belong to which attempt.** A downlink row belongs to the last uplink row sent at or before it (ties by row order). A downlink row that comes before every request, or before its request's receipt, or answers a lost request, is an error. The last answer message is the latest sent, ties by row order. Rows are clipped to their leaf before anything else, and an inverted row (received before sent) is an error before clipping.
- **A local tool carrying link rows is an error.** The ingester already refuses link spans under anything but a chat or a remote tool, so this is a second guard.
- **ATR-14 runs on every turn of every bundle.** Two checks against the turn view: the tool leaves' total time, and the chat leaves' total time less their delay and rate-limit time. Both also hold on real `sim` bundles (`crates/acn-hyp/tests/attribution.rs`), where a constant delay `d` gives exactly `2d` per call.
- **ATR-15 in the verdict.** `BundleData.attrib` holds either the bundle's split or why it failed. A verdict refuses only when one of its measures is an attribution quantity, so a POC 4 verdict is never blocked by attribution.
- **The table gains ten quantities.** `network_attributable_share`, `model_share`, `tool_share`, `retry_share` and `other_share` (ATR-20), and `tail_<cause>_share_p99` (ATR-21). Each is computed by `acn_attrib::core::share` or `tail_share`, never in `acn-hyp`. `lab/hypotheses/p17-a2a.toml`'s `network_attributable_share` now resolves; its other quantities stay warnings.
- **Not in this PR.**
  - ATR-40 and ATR-41 come with T15.3.
  - ATR-42 comes with T16's evidence pages.
  - The calibration suite `tests/accept/attrib.rs` comes with T15.3, as a Class B suite over this core. The `2d` calibration on real bundles is already in `acn-hyp`'s tests here.
- **The frozen set moves.** `crates/acn-hyp/` and `crates/acn-attrib/src/core/` changed, so `env-hash.json` is rewritten and `engine_hash` moves (CON-28). Every `run_id` changes with it.

## After review (T15.2: adversarial review and cross-review)
Two read-only reviews ran on the uncommitted change: the Class C adversarial prompt of TASKS.md and a cross-review against SPEC 090. Their findings, and what was done:
- **The frozen boundary had two holes (blocking).** The maintainer chose to extend CON-7 in this PR (October 2026).
  - `acn-attrib`'s `lib.rs` and `Cargo.toml` were outside the hash, so an edit there could swap the core that verdicts compile without moving `engine_hash`.
  - The critical path's leaves come from `acn-trace`'s ingester, which was not frozen either. A one-line change there could move time between causes while every check still passed. The views have always depended on the same unfrozen code.
  - CON-7's list therefore now names `crates/acn-attrib/` whole and `crates/acn-trace/src/ingest/`. The ingester moved from `ingest.rs` to `ingest/mod.rs`, because the frozen set, its hash, CODEOWNERS and their tests all work with directories.
  - CON-28 names the ingester among the engine's code. `FROZEN_SET`, `.github/CODEOWNERS`, the `xtask` tests, CLAUDE.md's Class C line and PLAN.md follow suit.
  - Every later change to the ingester is Class C, and moves `engine_hash` and every `run_id`. That is the price of making the views and attribution as trustworthy as the verdict.
- **Plots leave `acn-attrib`.** A frozen crate cannot hold drawing code, or a cosmetic change would move `engine_hash`. So the heatmap is drawn by `acn-cli` (ATR-41), and CLAUDE.md's rule now reads "Plots come from `acn-cli` over `acn-attrib`'s numbers". SPEC 090 v0.2 says this, and drops the `plots` feature.
- **An overflow is an error everywhere (blocking).**
  - `share` and `tail_share` return `Result<Option<f64>, AttribError>`. `None` means only "no turn" or "no time".
  - A verdict reads attribution quantities through `quantities::attribution_value` and refuses on an error, citing ATR-30. `quantities::value` stays an `Option` for other callers.
  - A tail percentile outside 1 to 100 is an error.
- **Ownership on recorded times.** Clipping to the leaf turned distinct send times into ties, which row order then broke, and it could hide an answer sent before any request. Attempts and owners are now found on the recorded times, and clipping is only for measuring (SPEC 090 v0.2, ATR-11).
- **Rows are checked before use.**
  - A dropped row whose interval is not empty is an error.
  - An answer to a lost request has its own message.
  - Each message names the row (`link/direction sent at t`).
  - A zero-length leaf has zero parts, whatever rows it carries, once the rows themselves are well-formed.
- **Ties are stated and tested.** Of answers sent together, the last in row order is the last message. Of requests sent together, the last in row order is the last attempt. A test also rotates and reverses rows with distinct send times and requires the same split.
- **Tests that could pass on a regression were tightened.**
  - The ATR-15 refusal now comes from a real `decompose` error, and must name the bundle, the turn and both rules.
  - A replicate whose sums overflow must refuse.
  - The "good turn does not save the bundle" case now fails on ATR-14 and names turn 1.
  - The ATR-2 test takes the chat link wait from the link view.
  - A boundary test pins `acn-attrib`'s workspace dependencies to `acn-trace`, with no `plotters` and no build script.
  - The rank is tested at `p` = 1 and 50, not only 99.
  - ATR-31 is cited by the engine-hash test, which shows that an edit to `acn-attrib`'s manifest or crate root moves `engine_hash`.
- **Smaller fixes.**
  - `read::attribution` was renamed `decompose_views`, since `quantities::attribution` is unrelated.
  - `ViewBatches` is used in both read signatures.
  - The `turn_order` doc comment is back on its function.
  - Dead fields are gone, and ownership uses indices instead of pointer identity.
  - CON-18 is no longer cited by an integration test; the calibration suite that carries it comes with T15.3.
- **Left as is.**
  - Attribution runs on every bundle a verdict reads, even when no measure needs it. The cost is one walk per bundle, and computing it lazily would add a second path to keep in step.
  - In `decompose.rs` the ATR-14 check builds its expected turn-view row with the same formula. The independent check is the real `sim` bundle in `acn-hyp`'s tests.
