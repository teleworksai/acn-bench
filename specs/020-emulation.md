# SPEC 020 — Link emulation: models, engine, proxy and measured traces

**Status:** Draft v0.3 (October 2026; v0.3: the sim engine, §4, T11; v0.2: trace-driven links, EMU-10 to EMU-12 and EMU-22, T10b). Partial: §2, link models, and §3, scenario files, are written for T10 and T10b; §6, measured traces, for T08. §4, the sim engine, is written for T11 (implemented across T11.1 to T11.3). The live proxy (§5, T12, numbered from 40) is to write. **Inherits:** SPEC 000, 010. **Prefix:** EMU. **Crate:** `acn-emu` (Class B); `scenarios/synthetic/` is Class A, and `scenarios/measured/` is in the frozen set (CON-7).
**Purpose:**
- define a link model: what one direction of a link does to each message, as a pure function of its parameters, its random sub-stream and the messages sent so far (§2);
- define the scenario file that names a run's links (§3);
- define what a measured impairment trace is, so that CON-21's provenance rule and its ban on payload and identifiers can be checked by a machine, and how the first trace, a published 5G drive test (ADR-29), enters the repository (§6).

## 2. Link models

A *link* is one direction (`up`: client to server, or `down`) of the path between two endpoints. It carries *messages*: units of application data (an HTTP request, a response body, one streamed chunk) with a size in bytes. The sim engine (T11) and the live proxy (T12) both drive the same link model; this section fixes what the model computes.

**EMU-1** A link model MUST be a deterministic function of its parameters, its random sub-streams (EMU-9) and the sequence of messages offered to it. It MUST take each message as `(send_ns, bytes)`, in non-decreasing order of `send_ns`, and MUST refuse a message sent earlier than the one before. `send_ns` is measured from the link's origin, the start of the replicate the link belongs to, and outage windows (EMU-7) are on the same scale. Every time a link handles is an integer number of nanoseconds in `[0, 2⁶²]`, and a parameter or a message that would take a time outside it MUST be refused with reason `range`.

For each message the model MUST return its *fate*:
- delivered at `deliver_ns`, or dropped with a cause (`outage`, `loss` or `queue`);
- the outage window it met (its index), if any, and the time it was held there (`hold_ns`);
- the time it waited for the rate limiter (`rate_wait_ns`);
- the delay applied after the rate stage (`delay_ns`);
- whether the reorder stage selected it.

A delivered message satisfies `deliver_ns = send_ns + hold_ns + rate_wait_ns + delay_ns`. These are the quantities TRC-15 and TRC-34 record. Their conversion to the milliseconds of TRC-15's attributes is the recorder's (T11, T12).

**EMU-2** A link is a fixed pipeline of optional stages, applied in this order: outage (EMU-7), loss (EMU-5 or EMU-6, at most one), rate (EMU-4), delay (EMU-3), reorder (EMU-8). A message dropped by a stage is not offered to the stages after it, though their draws for it are still made (EMU-9). A link with no stages delivers every message at its `send_ns`.

**EMU-3** **Delay.** The delay stage has a base `delay_ns ≥ 0` and a `jitter_ns ≥ 0`. A message leaving the rate stage at `t` (or reaching this stage at `t`, when the link has no rate stage) gets a *candidate* time `max(t, t + delay_ns + j)`, where `j` is the stage's draw for it (EMU-9), uniform over the integers in `[-jitter_ns, jitter_ns]`. A message the reorder stage does not select is delivered at `max(candidate, f)`, where `f` is the delivery time of the last earlier message that was delivered and not selected: so such messages leave in order. The applied `delay_ns` of EMU-1 is the delivery time minus `t`.

**EMU-4** **Rate.** The rate stage is a token bucket with a rate `rate_bps > 0` (bits per second), a bucket `burst_bytes > 0` and a queue limit `queue_bytes > 0`.
- The bucket holds at most `burst_bytes` of credit and starts full; credit accrues continuously at `rate_bps`, up to that cap.
- Messages leave in order. A message of `b` bytes arriving at `t` leaves at the earliest time `t' ≥ t`, and no earlier than the message before it left, at which the credit is at least `min(b, burst_bytes)`. It then takes `b` bytes of credit, which may leave the credit negative. So a burst within the bucket leaves at once, and a long run settles at `rate_bps`.
- The *backlog* at `t` is the bytes of the accepted messages whose departure is later than `t`. A message that would leave later than `t` (`t' > t`) and would make the backlog exceed `queue_bytes` MUST be dropped with cause `queue` (tail drop). It takes no credit and does not move the departure order. A message that leaves at once never joins the backlog, whatever its size.
- The stage MUST compute in exact integers: credit is kept in units of bit-nanoseconds (one byte is `8 × 10⁹` units, and the bucket gains `rate_bps` units per nanosecond), and `t'` is rounded up to the next nanosecond. `rate_wait_ns` is `t' − t`.

**EMU-5** **Independent loss.** The loss stage drops a message with probability `loss_ppm / 10⁶`, decided by its draw (EMU-9), with cause `loss`.

**EMU-6** **Burst loss (Gilbert–Elliott).** The stage has two states, `good` and `bad`, and four parameters in parts per million: `p_good_bad` and `p_bad_good` (the per-message transition probabilities), and `loss_good` and `loss_bad` (the loss probability in each state). It starts in `good`. For each message offered to the link it first makes the transition with its first draw, then decides loss with its second, using the state it is now in. A message dropped earlier (by an outage) still moves the state, so the state sequence is fixed by the message index alone.

**EMU-7** **Outages.** The stage has a list of windows `[start_ns, end_ns)`, sorted, non-empty and non-overlapping (adjacent windows are allowed), each with a mode: `drop` drops a message sent inside the window with cause `outage`; `hold` delays it to `end_ns`, as if sent then, and the stage then applies again at `end_ns`, so a message held into an adjacent window meets that window too. Each window also has a cause for TRC-15's `acn.scenario.outage` event: `handover` or `scheduled`. The fate records the last window the message met.

**EMU-8** **Reorder.** The reorder stage has `reorder_ppm` and a `gap_ns > 0`. Each message is *selected* with probability `reorder_ppm / 10⁶`, by its draw (EMU-9). A selected message that is delivered is delivered at `max(candidate, f) + gap_ns`, with `candidate` and `f` as in EMU-3, so it arrives after every earlier message that was not selected, and later messages may overtake it. It does not become the `f` of later messages. TRC-15's `acn.link.reordered` is true for a selected message.

**EMU-9** **Draws and the impairment schedule.** All randomness of a link MUST come from per-stage sub-streams under the replicate seed (CON-30(b)), named `link.<name>.<direction>.<stage>` with `<stage>` one of `loss`, `delay` and `reorder`.
- **One schedule entry per message index.** Every stage present MUST take the same number of draws for every message offered to the link, by the message's index, whether or not an earlier stage dropped it:
  - independent loss and reorder: one draw each;
  - Gilbert–Elliott: two draws (transition, then loss);
  - delay: one draw when `jitter_ns > 0`, none otherwise;
  - the outage and rate stages draw nothing.
- **What that makes the draws.** They form a schedule fixed by the seed and the message index before any traffic, the impairment schedule of CON-5(d). With the outage windows, it determines every fate from the send times and sizes alone. Changing one stage's parameters changes no draw of another stage, and no message's draws in its own stage.
- **The draws themselves.** A uniform draw over `0..n` takes 64-bit outputs `x` of the stage's generator (`next_u64`) until `x < z`, with `z = (2⁶⁴ − 1) − ((2⁶⁴ − 1) mod n)`, and returns `x mod n`. A probability in parts per million is true when one uniform draw over `0..10⁶` is below the parameter. The jitter `j` is a uniform draw over `0..2·jitter_ns + 1`, minus `jitter_ns`. No floating point is used.

**EMU-10** **Time-varying parameters.** The loss (independent only), rate and delay stages MAY take their parameters from a *schedule*: a list of segments, each in force over `[from_ns, to_ns)` on the link's time scale, with one value of each parameter. A static stage is one segment in force at all times. Each stage uses the segment in force when the message reaches it: the loss stage at the send time plus any hold, the rate stage as EMU-11 says, the delay stage at the time the message leaves the rate stage. An outage segment (EMU-12) drops every message sent inside it, with cause `outage`.

**EMU-11** **Rate across segments.** Credit accrues at the rate in force at each instant, integrated exactly over the segments, and still up to the bucket's cap. A segment with rate 0 accrues nothing. The departure of EMU-4 is the earliest time at which the credit, so accrued, reaches the threshold, rounded up to the next nanosecond. The schedule of a trace repeats (EMU-12), and the loader refuses a trace with no positive rate in a direction (EMU-12), so every departure is finite. A departure beyond 2⁶² ns is refused with reason `range`.

**EMU-12** **Trace-driven link.** A link with `[link.trace]` takes its loss, rate and delay schedule from a measured trace (§6), replayed sample by sample:
- **Timing.** Sample `k` is in force over `[t_k, t_{k+1})` of trace time, however long that gap is, and the last sample for `sample_interval_s` after its `t_s`. The trace's *period* is the last `t_s` plus `sample_interval_s`. Times convert to nanoseconds as `t_s × 10⁹` and `(last t_s + sample_interval_s) × 10⁹`, each computed in `f64` and rounded as below. Link time `τ` reads trace time `(τ + start) mod period`, where `start` is the link's `start_s` offset. So the trace repeats for as long as the run lasts.
- **Values.** Each value converts with rounding to the nearest integer, ties away from zero, from the sample's `f64`:
  - one-way delay: half the round-trip average (`rtt_ms.avg × 10⁶ / 2` ns);
  - jitter: a uniform half-range of `rtt_ms.stdev × 10⁶ × √3 / 2` ns, so that each direction's jitter has half the round trip's standard deviation (a uniform over `[−a, a]` has standard deviation `a / √3`);
  - independent loss: `loss × 10⁶` ppm;
  - rate: `ul_kbps` for an `up` link or `dl_kbps` for a `down` link, times 1 000, in bits per second.
- **Outages.** A sample whose `loss` is 1, or whose rate in the link's direction is 0, is an outage segment: messages sent in it are dropped with cause `outage`, its rate is 0 (queued messages wait it out), and TRC-15's outage event takes cause `trace`. Its delay and jitter are those of the nearest earlier sample with a round-trip time, or of the first later one, so a queued message that leaves just as an outage begins is still delayed. A segment is an outage exactly when its rate is 0.
- **What the trace does not give.** The bucket and queue come from the scenario (`burst_bytes`, `queue_bytes`), since a trace measures neither.
- **No other stages.** A trace link MUST NOT also have an outage, loss, rate or delay stage. It MAY have a reorder stage.
- **Draws.** Its loss and delay stages draw once per message index, whatever the segment's parameters, so the schedule of draws stays fixed by the seed (EMU-9). Loss uses EMU-9's draw over `0..10⁶`. Jitter cannot use EMU-9's rejection draw, whose number of generator outputs depends on the range, and the range here depends on the segment, so on timing. Instead it takes one 64-bit output `x` and uses `j = ⌊x · (2·jitter_ns + 1) / 2⁶⁴⌋ − jitter_ns`. Its bias is below `(2·jitter_ns + 1) / 2⁶⁴`, under 2⁻²⁰ for any jitter below 2⁴³ ns.
- **The fate** also records the trace sample in force when the message was sent. That sample decides the outage and the loss; the delay comes from the segment in force when the message leaves the rate stage, which may be a later one.

## 3. Scenario files

**EMU-20** A synthetic scenario MUST be one TOML file `scenarios/synthetic/<name>.toml`, parsed with unknown keys refused at every level. Its hash is the BLAKE3 of the file's bytes (CON-27(a)).
- It MUST hold `schema_version = 1`, a `name` equal to its file stem, and one or more `[[link]]` tables.
- Each link has a `name`, a `direction` (`up` or `down`) and one optional table per stage of EMU-2.
- Names are lowercase ASCII letters, digits and `-`. No two links may share a name and a direction.
- The keys are fixed, each integer with its unit in its name:
  - `[link.delay]`: `delay_us`, `jitter_us`;
  - `[link.rate]`: `rate_kbps` (1 000 bits per second), `burst_bytes`, `queue_bytes`;
  - `[link.loss]`: `kind = "iid"` with `loss_ppm`, or `kind = "gilbert_elliott"` with `p_good_bad_ppm`, `p_bad_good_ppm`, `loss_good_ppm`, `loss_bad_ppm`, and no key of the other kind;
  - `[[link.outage.window]]`: `start_ms`, `end_ms`, `mode` (`drop` or `hold`), `cause` (`handover` or `scheduled`);
  - `[link.reorder]`: `reorder_ppm`, `gap_us`;
  - `[link.trace]`: `dir`, `blake3`, `start_s`, `burst_bytes`, `queue_bytes` (EMU-22).

  The loader converts every duration to nanoseconds.

**EMU-21** `acn_emu::scenario::load` MUST refuse a scenario that breaks EMU-20, with one of these reasons:
- `layout`: the file cannot be read, or is not a `.toml` file;
- `parse`: not TOML of the EMU-20 shape, including an unknown or missing key;
- `name`: a name that is not a name, or a `name` other than the file stem;
- `duplicate`: two links with one name and direction;
- `range`: a parameter out of its range, such as a probability above 10⁶, a zero rate, burst or queue, a zero reorder gap, an outage window that is empty, unsorted or overlapping, or a duration beyond 2⁶² ns;
- `trace`: EMU-22.

It MUST return the scenario's hash and its link parameters. `Scenario::build` MUST then build each link's model (EMU-1) under a given replicate seed.

**EMU-22** A link that names a measured trace (§6) does so in `[link.trace]`, with these keys:
- `dir`: the trace's directory, relative to the directory holding the scenario file (`../measured/<slug>` from `scenarios/synthetic/`), or absolute;
- `blake3`: the BLAKE3 of its `trace.toml`;
- `start_s`: an integer offset in seconds into the trace, at least 0 and below its period; optional, 0 when absent;
- `burst_bytes` and `queue_bytes`.

The loader MUST load the trace under EMU-64 and MUST refuse it, with reason `trace`, when the trace does not load, when the hash differs (CON-27(a)), when `start_s` is outside the period, or when the trace has no positive rate in the link's direction. It MUST refuse a trace link that also has an outage, loss, rate or delay stage, with reason `parse`, before it reads those stages.

## 4. The sim engine

The sim engine runs a replicate's traffic over a scenario's links on virtual time (CON-5(c)). It has two parts: an event queue, which the harness's scheduler is built on, and a *network*, which carries a call's request and response across a path. T11.1 implements both in `acn_emu::sim`; T11.2 rebuilds the harness's sim scheduler on the queue; T11.3 wires a scenario into `acn run` and emits the spans of EMU-36 and EMU-37.

**EMU-30** **The event queue.** Events are ordered by `(time_ns, seq)`, where `seq` is the order in which they were added. The queue's clock starts at 0 and moves only forward, to the time of the earliest pending event. An event for a time earlier than the clock MUST be refused.

**EMU-31** **Groups.** The queue MUST hand out every event due at the earliest pending time as one group, in `seq` order, and only then move on. The harness takes calls due at one instant from one group, which is what lets it submit them together (HAR-41).

**EMU-32** **Paths and messages.** A *path* is the pair of links of one name in a scenario, `up` and `down`, and every call crosses exactly one path. A call's request is one message on the uplink, sent when the call is made. Its response is one or more messages on the downlink: a body that is not streamed is one message of the body's bytes, sent when the server responds. Each event of a streamed response is one message of its wire bytes (`data: …` and the blank line), sent at the time the server emits it. A response message is offered to the downlink when the network's clock reaches its send time, so the messages of concurrent calls meet the link in time order (EMU-1), and messages with the same send time in the order the responses were registered.

**EMU-33** **Arrival.** The server MUST see a request at its uplink delivery time, not its send time. Requests delivered at the same instant MUST reach the server together, in the order they were sent (HAR-41, MLM-7).

**EMU-34** **Order within a response.** A response's messages are received in order: a message is received at the later of its delivery time and the time the message before it in the same response was received. A response is received when its last message is.

**EMU-35** **Drops.**
- **A dropped request or body.** The call's response never arrives, so the call ends at its deadline as a timeout.
- **A dropped event of a streamed response.** The events before it are received. The stream ends with a transport failure when the next event of that response would have been received, or at the deadline as a timeout if there is no later event.

**EMU-36** **Link spans.** In a run with a scenario, every message MUST be recorded as an `acn.link` span (TRC-15) under the call it belongs to, with these attributes:
- `enqueue_ns`: the send time;
- `dequeue_ns`: the delivery time, or the send time for a dropped message;
- `applied_delay_ms`: the hold and the delay of EMU-1, in milliseconds;
- `rate_limited_ms`: the rate wait;
- `dropped` and `reordered`;
- the link's name and direction.

**EMU-37** **The scenario span.** A run with a scenario MUST record one `acn.scenario` span (TRC-16), with ids from the run's `trace.ids` stream (TRC-27). It carries the scenario file's bytes and hash, and an `acn.scenario.outage` event for each outage window or trace outage segment that a message met, with cause `handover`, `scheduled` or `trace`.

**EMU-38** **Determinism.** The same seed, scenario and sequence of offered calls MUST give the same fates, the same groups and the same receive times, on every machine (CON-5(c)).

**EMU-39** **No scenario, no network.** A run with no scenario has no network: calls go straight to the server, as before T11. It MUST record a zero `scenario_hash` and no link or scenario span, and MUST draw nothing for the network, so its bundles are byte-identical to the same run before T11.

## 6. Measured traces

**EMU-60** A measured trace MUST be a directory `scenarios/measured/<slug>/`, with `<slug>` lowercase ASCII letters, digits and `-`, holding exactly two files: `trace.toml` and `provenance.toml`. Both MUST be regular files, not links. Both are parsed with unknown keys refused at every level. `trace.toml` is the run input: scenarios name a trace by its hash (EMU-64), read as working-tree bytes (CON-27(a)). `provenance.toml` is not read by a run; it is committed by the frozen set's `env-hash` (CON-7), as the trace is.

**EMU-61** `provenance.toml` MUST state:
- the source: a title, a URL, and a persistent identifier (DOI or record number) when there is one;
- the licence as an SPDX identifier, and the attribution the licence asks for;
- the collection date (`YYYY-MM-DD`), the device, the network technology, and whether the network is a testbed or commercial;
- the location class (for example `suburban road`), never coordinates;
- the mobility (`static`, `walk` or `drive`);
- the method: how the source measured;
- the conversion:
  - the tool that turned the source into `trace.toml`, as a path in this repository and the BLAKE3 of that source file (a hash, not a commit, because the conversion lands in the same change as the trace);
  - the name and BLAKE3 of every source file it read;
- what the conversion dropped, and why: at least one entry.

A licence that does not allow redistribution MUST be refused. Source file names MUST be non-empty and distinct, and every BLAKE3 lowercase hex. A text field that is present MUST NOT be empty.

**EMU-62** `trace.toml` MUST hold `schema_version = 1`, a positive `sample_interval_s` (the nominal spacing of samples), and an ordered array of `[[sample]]` tables. A sample MUST carry only these fields:
- `t_s`, seconds since the first sample, which is 0. It is strictly increasing.
- `loss`, a fraction in `[0, 1]`.
- `rtt_ms`, a table `{ avg, min, max, stdev }` with `0 ≤ min ≤ avg ≤ max` and `stdev ≥ 0`. It is present exactly when `loss < 1`: a sample in which no probe returned has no round-trip time, and still belongs to the trace (an outage is the impairment a trace exists to record).
- `dl_kbps` and `ul_kbps`, the throughput measured downlink and uplink, each ≥ 0.

Every number MUST be finite. A trace MUST hold at least two samples.

**EMU-63** A measured trace MUST NOT contain payload data or identifiers (CON-21). For the samples, the fixed field set of EMU-62 enforces this: no coordinates, no cell or device identifier, no address and no wall-clock time. Times are relative to the first sample. The free text the format allows (comments in either file, and the descriptive fields of EMU-61) cannot be checked by a machine; keeping identifiers out of it is an obligation of the reviewer of the change that adds the trace (CON-7).

**EMU-64** `acn_emu::trace::load` MUST read a trace directory and refuse it with a named reason (`layout`, `parse`, `samples`, `range`, `licence` or `provenance`) when either file breaks a machine-checkable rule of EMU-60 to EMU-63. It MUST also return the trace's hash: the BLAKE3 of `trace.toml`, which scenarios will name it by (CON-27(a)).

**EMU-65** The first measured trace (ADR-29) is `scenarios/measured/5g-iana-2023-01-29/`, converted from the PING file of Zenodo record 12664724 (CC BY 4.0). Its conversion tool is `cargo xtask import-5g-iana`. Run on the published source files, the tool MUST reproduce `trace.toml` byte for byte. It MUST refuse:
- a source whose BLAKE3 differs from the one recorded in `provenance.toml`;
- to run when its own source file's BLAKE3 differs from `conversion.tool_blake3`, so that a change to the tool forces a re-conversion and a change to the frozen set;
- a source off the published structure, rather than guess;
- output that breaks EMU-62, rather than write it.

## 9. Acceptance tests

- `crates/acn-emu/tests/sim_engine.rs` — EMU-30 to EMU-35, EMU-38 (T11.1):
  - the queue's order, groups and refusal of the past;
  - a request delivered later than sent, and requests delivered at one instant;
  - response messages of concurrent calls offered to the downlink in time order;
  - the order rule within a response;
  - each drop of EMU-35;
  - a golden vector of a network's events on `cellular-handover`;
  - two runs on `5g-iana-replay` giving identical results.
- The harness's acceptance suites (T11.2, T11.3) — EMU-36, EMU-37, EMU-39: a run with no scenario unchanged byte for byte, a scenario run twice byte-identical, `link.parquet` filled, a timeout recorded for a drop.

- `crates/acn-emu/tests/link_models.rs` — EMU-1 to EMU-9:
  - **The statistics of each stage**, over many messages and several seeds:
    - the loss rate of EMU-5 within four standard deviations of `loss_ppm`;
    - the stationary loss and mean burst length of EMU-6 against their closed forms;
    - jitter within its range, with the mean of the uniform;
    - throughput settling at `rate_bps`, a burst within the bucket leaving at once, and tail drop at the queue limit;
    - the reorder fraction against `reorder_ppm`.
  - **The structure**: FIFO delivery without reorder, outage `drop` and `hold`, the stage order of EMU-2, the refusal of a message sent out of order, the index-aligned schedule of EMU-9 (changing the loss rate leaves every delivered message's jitter unchanged, and changing the jitter leaves the losses unchanged), each stage's edge cases (the bucket cap after idle, the backlog boundary, a tail drop taking no credit, a message larger than the queue leaving at once, adjacent windows, the reordered message's exemption, the clamp at `t`), the exact draw algorithm, and refusals of out-of-range times, and a golden vector of fates for one seed.
- `crates/acn-emu/tests/trace_link.rs` — EMU-10 to EMU-12, EMU-22:
  - the schedule built from the 5G-IANA trace: timing, values, rounding, outages and their carried delay, the period and `start_s`;
  - a zero rate as an outage;
  - rate integrated across segments, a queue waiting through an outage, and whole periods skipped;
  - a departure exactly at an outage's start keeping the carried delay;
  - draws by index across a changed outage and a changed loss;
  - a golden vector of trace-link fates for one seed;
  - the replay scenario running over two periods;
  - the refusals of a trace reference: another hash, a missing trace, an offset outside the period, extra stages, no positive rate.

  Static links are unchanged: the golden vector of `link_models.rs` holds.
- `crates/acn-emu/tests/scenario.rs` — EMU-20 to EMU-22: every scenario under `scenarios/synthetic/` loads and reads as written, field by field; each malformed variant is refused by name, including a trace link with another stage.

### §6

- `crates/acn-emu/tests/measured.rs` — EMU-60 to EMU-64:
  - every trace under `scenarios/measured/` loads, and the 5G-IANA trace holds its 198 samples and 19 outages;
  - each malformed trace is refused by name: an unknown key, a coordinate field, a wrong schema version, a first `t_s` that is not 0 (including `-0.0`), a decreasing or repeated `t_s`, `min > avg`, `max < avg`, a negative `min`, `stdev`, loss or throughput, a loss above 1, a non-finite number, an incomplete `rtt_ms`, a zero `sample_interval_s`, a single sample;
  - the outage rule is checked both ways: `rtt_ms` missing below a loss of 1, and present at a loss of 1;
  - each broken provenance is refused by name: a non-redistributable licence, a missing field, an unknown mobility or network, an empty attribution or identifier, a coordinate field, a malformed hash, a date that is not a date, no sources, a duplicate source, nothing dropped;
  - a broken layout is refused: a missing file, an extra file, a symlink, a name that is not a slug.
- `crates/xtask/tests/import_5g_iana.rs` and the module's unit tests — EMU-65:
  - the importer, run on an excerpt with the published structure, writes the documented bytes, then `--check` passes, and fails once the bytes are changed;
  - it refuses a source with another hash, a changed tool, `--check` with no trace, a source off the published shape, and a source whose output would break EMU-62;
  - the committed provenance names the tool by its current hash.
- The byte-for-byte reproduction from the published source is checked by hand: the `#[ignore]` test `the_committed_trace_is_the_importers_output_on_the_source`, with `ACN_5G_IANA_PING` naming the downloaded file. The source is not in the repository (EMU-63), so no CI tier runs it; the test returns early without the variable.

## 10. Open questions (ADR candidates)

1. ~~How a scenario uses a measured trace.~~ Settled by T10b (ADR-33): replayed sample by sample (EMU-10 to EMU-12). Fitting a trace to a Gilbert–Elliott model, for a scenario that wants the trace's statistics without its timeline, is a later option.
2. Whether `scenarios/measured/` should also hold our own captures (T08's original plan, a phone-tethered walk) in the same format, with `mobility = "walk"`. That is expected; the format does not depend on the source.
3. How the live proxy (§5, T12) cuts a byte stream into the messages of §2: by HTTP message, by streamed chunk, or by write. TRC-15 allows a message or a byte segment.
