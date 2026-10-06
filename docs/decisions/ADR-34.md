# ADR-34 — T11: the sim engine, in three PRs

**Status:** accepted (T11.1; `spec-change`, SPEC 020 Draft v0.3 §4; Class B, `acn-emu`). **IDs affected:** EMU-30 to EMU-39; HAR-41 (SPEC 040 v0.3); CON-5(c), CON-29; TRC-15, TRC-16, TRC-27.

## Context
TASKS.md's T11 asks for:
- a discrete-event engine on the sim clock;
- a message-level transport;
- a test that sim runs are bit-identical.

The harness already runs `sim` on its own deterministic scheduler (`SimEnv`, HAR-41), which hands calls straight to the in-process mock with no network in between. Today no run has a scenario: `scenario_hash` is zero everywhere (ADR-17), and SPEC 085's loop states that convention.

## Decision
- **Three PRs, one concern each.**
  - **T11.1** (this one) writes SPEC 020 §4 and adds `acn_emu::sim`: an event queue and a network. The harness does not change.
  - **T11.2** rebuilds the harness's scheduler on the queue. It is a refactor whose proof is that bundles do not change.
  - **T11.3** adds `acn harness run --scenario`, carries calls over the network, and emits the link and scenario spans (EMU-36, EMU-37, EMU-39).
- **No scenario stays the default (EMU-39).** `clean.toml` hashes to a non-zero value. Making it the default would change every `run_id`, the POC 4 loop's id, and the loop's pins check, so it would orphan the run of record `f3afab22…`.
  - A run with no scenario has no network, draws nothing for one, and emits no span for one. So every file of its bundle stays byte-identical, except those that record the build: `manifest.json` and the build attributes of `resources.parquet` carry `build_hash`, which covers every file under `crates/` (CON-31) and so changes with any code change. A first draft promised full byte-identity, which no code change can keep; the review caught it. T11.2 pins the digest of the other files.
  - `clean` is an explicit baseline, for a treatment that impairs the link.
- **`acn loop run` gets no scenario option in T11.** Its `run_id` prediction and pins check (`crates/acn-hyp/src/loop_run.rs`) hard-code the zero hash. `crates/acn-hyp` is in the frozen set (CON-7), so changing it is an `env-change` PR with an adversarial review that the maintainer merges. It also moves `engine_hash`, and with it every loop's id. That PR comes when a POC first needs a network in a loop.
- **The engine lives in `acn-emu`**, so the live proxy (T12) can share its queue and its path pairing.
- **One path per scenario, for now (EMU-32).** Every call crosses the scenario's one path. Mapping calls to several paths (per tool, per sub-agent) waits for a POC that needs it.
- **Messages (EMU-32).**
  - A request is one message.
  - A response is one message, or one per SSE event of its wire bytes.
  - Response messages are offered to the downlink when the network's clock reaches their send times, because the mock gives all of a call's chunk times up front, and the link must see concurrent calls' messages in time order (EMU-1).
- **Order within a response (EMU-34).** A response is one HTTP exchange over one connection, so its messages are received in order, as TCP would deliver them. A message delivered early waits for the one before it.
- **Drops are final (EMU-35).** The message-level model does not retransmit. A lost request or body ends the attempt at its deadline, which is HAR-24's: the smaller of the request timeout and the time left in the run. A lost stream event cuts the stream at the next event's receipt, unless the deadline comes first. Each attempt is its own call on the network, so a retry crosses the link again. The events received before the end are kept, and so are their bytes, so the call's downlink bytes agree with its link spans.
  - This is pessimistic next to TCP, which would recover a lost segment late rather than never. The live proxy (T12), over real TCP, will show how far apart they are, through the twin divergence of CON-25. Recovery can then be modelled if it matters.
- **One instant has three phases (EMU-33), and HAR-41 is amended now.**
  1. Wake the waits due at the instant.
  2. Run every lineage until nothing new happens at the instant, sending each request up its link.
  3. Hand every request delivered at the instant to the mock as one batch.

  A request over a zero-delay link therefore joins its own instant's batch rather than forming a second one, so `clean` batches as no scenario does. The batch's order is MLM-7's. A first draft said "in the order sent", which MLM-7 overrides. SPEC 040's HAR-41 now batches by the instant a call reaches the mock, in this PR, so main never holds the two specs in conflict (CON-13).
- **Receive times are the exchange's times (EMU-34).** The harness's `Exchange` events, from which time to first token and the gaps between tokens are measured, take the network's receive times, not the mock's send times. A body, or an error response, is sent at the mock's `respond_at_ns`.
- **How T11.3 records the spans (EMU-36, EMU-37).**
  - **Link spans come from an `acn-emu` resource.** The attribute registry lists `acn-emu` as the producer of `acn.link.*` and of the scenario events, and TRC-19 makes a producer a resource with its `service.name`. So link spans come from an `acn-emu` resource, not from a scope under the harness, and `acn-emu` appears in the manifest's producers.
  - **Ids stay deterministic.** The resource shares the replicate's `trace.ids` generator: a sim run is single-threaded, so the draws happen in program order. A first draft read "emitted by acn-emu" as a tracer scope, to avoid a second generator; sharing one generator makes that unnecessary.
  - **The scenario span belongs to the run.** Its ids are drawn from the run's stream before the first replicate, so every link span can carry the span link TRC-16 requires.
  - **Time fields.** The hold time of an outage counts inside `applied_delay_ms`, so the ingest's network-wait figure includes it. The millisecond fields are floats, as the registry types them.
  - **`acn.link.model`** names the link's stages in pipeline order, for example `trace+reorder`.
  - **Outage events.** They are derived from the fates: each window in which some message was dropped or held, and each trace outage segment in which some message was dropped. A queue's wait through an outage it was not sent in shows in that message's `rate_limited_ms`.

- **Scenario runs are exploratory until T12.** They have no live twin until then (CON-25), so their numbers are not cited.

## Consequences
- **What `acn_emu::sim` has to offer the harness.**
  - `EventQueue` hands out events in groups of one instant.
  - `Network` carries requests (with their delivery times) and responses (with their receive times), and says how each call ended.
  - Its lookups (`request_fate`, `messages`, `link`) give T11.3 what the spans need, and `forget` frees finished calls.
  - `respond` refuses a response for a request that was not delivered, and send times that decrease. It checks everything before it registers anything.
- **What T11.2 must show.** A pinned digest of a no-scenario run's bundle, build files aside, unchanged by the refactor.
- **What T11.3 must show.** A scenario run twice is byte-identical, and a drop surfaces as a timeout.
