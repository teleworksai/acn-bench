# ADR-34 — T11: the sim engine, in three PRs

**Status:** accepted (T11.1; `spec-change`, SPEC 020 Draft v0.3 §4; Class B, `acn-emu`). **IDs affected:** EMU-30 to EMU-39; HAR-41; CON-5(c), CON-29; TRC-15, TRC-16, TRC-27.

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
  - **T11.3** adds `acn run --scenario`, carries calls over the network, and emits the link and scenario spans (EMU-36, EMU-37, EMU-39).
- **No scenario stays the default (EMU-39).** `clean.toml` hashes to a non-zero value. Making it the default would change every `run_id`, the POC 4 loop's id, and the loop's pins check, so it would orphan the run of record `f3afab22…`.
  - A run with no scenario has no network, draws nothing for one, and emits no span for one, so its bundles stay byte-identical.
  - `clean` is an explicit baseline, for a treatment that impairs the link.
- **`acn loop run` gets no scenario option in T11.** Its `run_id` prediction and pins check (`crates/acn-hyp/src/loop_run.rs`) hard-code the zero hash. `crates/acn-hyp` is in the frozen set (CON-7), so changing it is an `env-change` PR with an adversarial review that the maintainer merges. It also moves `engine_hash`, and with it every loop's id. That PR comes when a POC first needs a network in a loop.
- **The engine lives in `acn-emu`**, so the live proxy (T12) can share its queue and its path pairing.
- **Messages (EMU-32).**
  - A request is one message.
  - A response is one message, or one per SSE event of its wire bytes.
  - Response messages are offered to the downlink when the network's clock reaches their send times, because the mock gives all of a call's chunk times up front, and the link must see concurrent calls' messages in time order (EMU-1).
- **Order within a response (EMU-34).** A response is one HTTP exchange over one connection, so its messages are received in order, as TCP would deliver them. A message delivered early waits for the one before it.
- **Drops are final (EMU-35).** The message-level model does not retransmit. A lost request or body ends the call at its deadline, and a lost stream event cuts the stream at the next event's receipt.
  - This is pessimistic next to TCP, which would recover a lost segment late rather than never. The live proxy (T12), over real TCP, will show how far apart they are, through the twin divergence of CON-25. Recovery can then be modelled if it matters.
- **HAR-41's "the same instant" becomes the arrival instant** once a scenario is in force (EMU-33). T11.3 clarifies SPEC 040 when it changes the harness.
- **How T11.3 records the spans (EMU-36, EMU-37).**
  - A message's fate travels back to the harness with the call's exchange, and the harness emits the `acn.link` span under the call's `chat` span, under the tracer scope `acn-emu`. A second tracer provider would draw the replicate's `trace.ids` stream in a second place, and the order of draws would then depend on scheduling.
  - TRC-15's "emitted by `acn-emu`" is read as that tracer scope.
  - The hold time of an outage counts inside `applied_delay_ms`, so the ingest's network-wait figure includes it.

## Consequences
- **What `acn_emu::sim` has to offer the harness.** `EventQueue` hands out events in groups of one instant. `Network` carries requests (with their delivery times) and responses (with their receive times), and says how each call ended.
- **What T11.2 must show.** A pinned digest of a no-scenario run's bundle, unchanged by the refactor.
- **What T11.3 must show.** A scenario run twice is byte-identical, and a drop surfaces as a timeout.
