# ADR-35 — T11.3: the harness on the network

**Status:** accepted (T11.3; `spec-change`, SPEC 020 §4 wording; Class B, `acn-harness`, `acn-cli`, `acn-trace` outside the frozen set). **IDs affected:** EMU-32 to EMU-37, EMU-39; HAR-24, HAR-41; TRC-15, TRC-16, TRC-19, TRC-27; CON-7, CON-29. **Follows:** ADR-34.

## Context
T11.1 gave `acn_emu::sim` a network, and T11.2 put the harness's scheduler on its queue. T11.3 makes a `sim` run's calls cross a scenario's network, and records the link and scenario spans. The work turned up a few things the spec did not settle.

## Decision
- **A separate entry point, `run_with_scenario`.** `RunConfig` gains no field, because `crates/acn-hyp/tests/common/exec.rs`, in the frozen set, builds it field by field: a new field would make this an `env-change`.
  - `acn harness run --scenario <file>` calls `run_with_scenario`. `run` stays the no-scenario call.
  - The CLI subcommand is `acn harness run`, not `acn run`, as SPEC 020's earlier text said.
  - `acn loop run` still has no scenario (ADR-34).
- **One instant's steps (EMU-33).**
  1. Wake the waits due at the instant, and in the same step end the attempts whose end is now known.
  2. Then send the calls made at the instant up the link.
  3. Then hand the requests delivered at the instant to the mock as one batch.
  4. Only then move the clock, to the next wait, arrival, message or deadline.
  - Waking and ending go together because without a network an attempt's end is itself a wait (the exchange sleeps until it), woken in the same group as every other wait due then. Two separate steps would let lineages resume, and so draw from the replicate's shared streams, in another order. The first implementation had them apart, and a test caught it.
- **An attempt ends as soon as its end is known.** The end is known:
  - at its last message's offer, for a success;
  - at the offer of the message after a lost one, for a cut;
  - at its deadline, for a timeout.

  The exchange then waits until that end, so a lineage never resumes late.
- **Messages after an attempt's end are not recorded (EMU-36).** After a timeout the mock's response still crosses the downlink, so it still shapes later fates there, but it belongs to no attempt and gets no span. SPEC 020's EMU-36 now says "every message an attempt carried by the time it ended".
- **How the spans are built.**
  - **Link spans** come from a per-replicate `acn-emu` provider that shares the harness's id generator through `SharedIdGenerator` (`acn-trace/src/ids.rs`). Their parent is the call's `chat` span, which is now ended after them, through its context.
  - **The scenario span** comes from a run-level `acn-emu` provider on the run's `trace.ids` stream. It is started at 0 before the first replicate, so every link span can link to it. Its provider allows unlimited events per span, because the SDK's default of 128 would drop events, and the collector refuses a span that dropped some.
- **Scenario events (EMU-37)** are gathered from every replicate's fates and recorded once each, since replicates share the scenario's timeline:
  - an outage event per window met (held or dropped);
  - an outage event per occurrence of a trace outage segment that dropped a message, over that occurrence's link times;
  - a step event the first time a message is sent in each trace sample, with the sample's values as JSON.
- **A whole-run comparison cannot show that a zero-delay path changes nothing.** The scenario's hash enters `run_id`, `run_id` enters every prompt's isolation marker (HAR-42), and the mock orders a batch by prompt hash (MLM-7). So two such runs answer a batch's identical calls in different orders. The equivalence is therefore tested on one session with one marker (`crates/acn-harness/tests/network.rs`), where every exchange, batches included, is the same.
- **A request's body is kept until it reaches the mock.** A body that was lost is dropped at once.

## Consequences
- `acn harness run --scenario scenarios/synthetic/cellular-handover.toml` runs a sim POC over a guessed cellular link, and `--scenario scenarios/synthetic/5g-iana-replay.toml` over the measured 5G drive. Both are exploratory until T12 gives them a live twin (CON-25).
- Bundles without a scenario are unchanged: the pinned digests of T11.2 still hold.
- The ingest already accepts the link and scenario spans (TRC-34's `link.parquet`), and `acn bundle verify --views` recomputes them.
