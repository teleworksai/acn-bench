# SPEC 020 — Link emulation: models, engine, proxy and measured traces

**Status:** Draft v0.4 (October 2026; v0.4: the live proxy, §5, T12; v0.3: the sim engine, §4, T11; v0.2: trace-driven links, EMU-10 to EMU-12 and EMU-22, T10b). Written for M1: §2, link models, and §3, scenario files, written for T10 and T10b; §4, the sim engine, for T11 (implemented across T11.1 to T11.3); §5, the live proxy, for T12 (implemented in T12.2 and T12.3); §6, measured traces, for T08. **Inherits:** SPEC 000, 010. **Prefix:** EMU. **Crate:** `acn-emu` (Class B); `scenarios/synthetic/` is Class A, and `scenarios/measured/` is in the frozen set (CON-7).
**Purpose:**
- define a link model: what one direction of a link does to each message, as a pure function of its parameters, its random sub-stream and the messages sent so far (§2);
- define the scenario file that names a run's links (§3);
- define how a run's calls cross those links, on virtual time in `sim` (§4) and through a proxy on real sockets in `live` (§5);
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

The sim engine runs a replicate's traffic over a scenario's links on virtual time (CON-5(c)). It has two parts: an event queue, which the harness's scheduler is built on, and a *network*, which carries a call's request and response across a path. T11.1 implements both in `acn_emu::sim`; T11.2 rebuilds the harness's sim scheduler on the queue; T11.3 wires a scenario into `acn harness run` and emits the spans of EMU-36 and EMU-37.

**EMU-30** **The event queue.** Events are ordered by `(time_ns, seq)`, where `seq` is the order in which they were added. The queue's clock starts at 0 and moves only forward, to the time of the earliest pending event. An event for a time earlier than the clock MUST be refused.

**EMU-31** **Groups.** The queue MUST hand out every event due at the earliest pending time as one group, in `seq` order, and only then move on. The harness takes calls due at one instant from one group, which is what lets it submit them together (HAR-41).

**EMU-32** **Paths and messages.** A *path* is the pair of links of one name in a scenario, `up` and `down`. A scenario used by `acn harness run` MUST have exactly one path, which every call crosses; a mapping of calls to several paths is for a later spec.
- **The request.** One message on the uplink, of the request body's bytes, sent when the attempt is made.
- **The response.** One or more messages on the downlink. A body that is not streamed, including an error response, is one message of its bytes, sent at the server's `respond_at_ns`. Each event of a streamed response is one message of its wire bytes (`data: …` and the blank line), sent at the server's emission time for it (`Chunk.at_ns`).
- **Order on the link.** A response message is offered to the downlink when the network's clock reaches its send time, so the messages of concurrent calls meet the link in time order (EMU-1), and messages with the same send time in the order the responses were registered. A response's send times MUST NOT decrease.
- **Retries.** Each attempt of a call (HAR-24) is its own call on the network, with its own request and response messages.

**EMU-33** **One instant.** In a run with a scenario, the server sees every delivered request at its uplink delivery time, even when the attempt that sent it has already ended (its response then crosses the downlink, recorded by no attempt). The harness's scheduler (HAR-41) works through each instant `T` in three phases:
1. it wakes every wait due at `T`, and in the same step ends every attempt whose end is now known (EMU-35), so that lineages resume in the order they would without a network;
2. it lets every lineage run until none makes a new request or wait at `T`, sending each request made at `T` up its link;
3. it hands every request whose delivery time is `T` to the server as one batch, through `Mock::handle_batch` with arrival time `T`.

The batch's order is MLM-7's, never the order of sending. A request delivered at `T` by a zero-delay link therefore joins the batch of `T`.

**EMU-34** **Order within a response.** A response's messages are received in order: a message is received at the later of its delivery time and the time the message before it in the same response was received. The harness MUST record those receive times, not the server's send times, as the response's timestamps: the events of `Exchange`, from which time to first token and the gaps between tokens are measured, and the end of the exchange.

**EMU-35** **Drops and deadlines.** An attempt's *deadline* is HAR-24's: its start plus the smaller of the request timeout and the time left in the run. An attempt ends at the earliest of:
- the receipt of its last message, as a success;
- for a streamed response that lost an event, the receipt of the next event delivered after the lost one, as a transport failure (a cut stream);
- its deadline, as a timeout, when its request or body was lost, or a stream lost its last events, or the receipt would come later.

The events received before the end are kept, and the attempt's downlink bytes are theirs. A cut stream is retried like any transport failure (HAR-24).

**EMU-36** **Link spans.** In a run with a scenario, every message an attempt carried by the time it ended MUST be recorded as an `acn.link` span (TRC-15) under the call's `chat` span. (A message the server sends after the attempt ended, say after a timeout, still crosses the link, and so still shapes later fates, but belongs to no attempt and is not recorded.) Link spans are produced under a resource with `service.name = "acn-emu"` (TRC-19), whose ids come from the same `trace.ids` generator as the replicate's harness spans, drawn in program order (TRC-27). Each span MUST carry a span link to the `acn.scenario` span (TRC-16) and these attributes:
- `acn.link.id`: the link's name in the scenario;
- `acn.link.direction`: `up` or `down`;
- `acn.link.model`: the link's stages in pipeline order, joined by `+` (for example `outage+gilbert_elliott+rate+delay+reorder`, `trace+reorder`), or `none`;
- `acn.link.bytes`: the message's size;
- `acn.link.enqueue_ns`: the send time;
- `acn.link.dequeue_ns`: the receive time (EMU-34), or the send time for a dropped message;
- `acn.link.applied_delay_ms`: for a delivered message, `(hold_ns + delay_ns) / 10⁶` as a float, where `delay_ns` includes the order rule's wait and any reorder gap; 0 for a dropped one;
- `acn.link.rate_limited_ms`: `rate_wait_ns / 10⁶` as a float;
- `acn.link.dropped` and `acn.link.reordered`.

**EMU-37** **The scenario span.** A run with a scenario MUST record one `acn.scenario` span (TRC-16) under the `acn-emu` resource of the run, with ids drawn from the run's `trace.ids` stream before the first replicate starts (TRC-27), so that every link span can link to it. It starts at 0 and ends at the latest end of any replicate's sessions. It carries the scenario file's bytes as `acn.scenario.toml` and its hash. Its events, in order of time and then of link name, direction and window:
- an `acn.scenario.outage` event (`start_ns`, `end_ns`, `cause`) for each outage window of EMU-7 in which some message was dropped or held, and for each trace outage segment (EMU-12, cause `trace`) in which some message was dropped, over the times that segment was in force;
- an `acn.scenario.step` event the first time a message is sent in each occurrence of a trace segment. Its `step` is `<link>.<direction>.<sample>`, and its `params` are the segment's values as JSON (`sample`, `loss_ppm`, `rate_bps`, `delay_ns`, `jitter_ns`, `outage`). In `views/link.parquet` (TRC-34), the step in force for a message is the latest step of its own link and direction.

Events at one time sort outages before steps. An outage event's times are clamped at 0, since a trace segment's first occurrence can begin before the replicate does. A message's fate names only the last window it met (EMU-7), so a message held in one window and then dropped in the adjacent one records the second window only.

Waits that a queued message spent in an outage it was not sent in are visible in its `rate_limited_ms`, not as an event.

**EMU-38** **Determinism.** The same seed, scenario and sequence of offered calls MUST give the same fates, the same groups and the same receive times, on every machine (CON-5(c)).

**EMU-39** **No scenario, no network.** A run with no scenario has no network: calls go straight to the server, as before T11. It MUST record a zero `scenario_hash`, no `acn-emu` resource and no link or scenario span, and MUST draw nothing for the network. So every file of its bundle is byte-identical to the same run before T11, except those that record the build (`manifest.json` and the build attributes of `resources.parquet`, CON-31), which change with any change to the code.

## 5. The live proxy

In `live` mode the same link models act on real sockets: the harness talks to its endpoint through a proxy that delays, holds and drops what crosses it, message by message, as the sim network does (§4). The proxy frames messages as EMU-32 does, so that a sim run and its live twin (CON-25) apply the same models to the same messages. T12.2 implements the proxy in `acn_emu::proxy`; T12.3 runs `acn harness run --mode live --scenario` through it.

**EMU-40** **One proxy per replicate.** A live replicate with a scenario MUST carry its calls through a proxy of its own: an HTTP/1.1 listener on the loopback interface that forwards to the run's endpoint over the scenario's one path. The proxy's links MUST be built from the replicate seed before it accepts its first connection, so that its draws, the impairment schedule of EMU-9, are fixed before traffic starts (CON-5(d)). Its origin, time 0 of the links, is the replicate's start on the run's clock (TRC-26). When the replicate ends, the proxy MUST close every connection, downstream and upstream, and abort every forward still in flight; the fates it decided before then count, and nothing after.

**EMU-41** **Messages.** The proxy MUST frame messages as EMU-32 does, and MUST refuse what it cannot frame:
- a request is one message of its body's bytes, offered once the body has been read in full; a request whose client gives up before that is not offered, and takes no message index;
- a response that is not a `200` event stream is one message of its body's bytes;
- each event of a `200` `text/event-stream` response is one message of its wire bytes, from its first line to the blank line that ends it, whatever its line endings; a block of comment lines only (`: …`) belongs to the event after it, and bytes left when the stream ends form a last message;
- headers and transfer framing (chunk sizes, `Content-Length`) are not counted;
- the proxy speaks HTTP/1.1 with keep-alive; a request in HTTP/2 is refused by closing the connection, and `CONNECT` or an `Upgrade` request by a `501` response, offered to no link.

**EMU-42** **Send times.** A message's send time MUST be the run's clock, less the origin, at the moment the proxy has read the whole message. Reading the clock and offering the message to its link MUST happen together, one message at a time per link, so that send times reach a link in non-decreasing order (EMU-1). A response's send time is thus the upstream's emission time plus the hop to the proxy (the loopback for the mock; the real network path for a provider).

**EMU-43** **Delivery.** The proxy MUST write a delivered message no earlier than its delivery time, and MUST write the messages of one connection in order. The time it finishes writing the message is its receive time in the proxy's records (EMU-47). Timer slack makes that later than the delivery time by up to a few milliseconds. The harness's own exchange timestamps (EMU-34) are its client's readings of the same clock as the bytes arrive, so they are no earlier than the proxy's.

**EMU-44** **Drops.** A dropped message cannot be cut out of a TCP stream, so the proxy MUST turn each drop into what the client of EMU-35 sees in `sim`:
- **A lost request.** It is not forwarded, and the connection stays silent, so the attempt ends at its deadline.
- **A lost non-streamed body.** The response is not written at all, and the connection stays silent.
- **A lost event of a stream, the first included.** The proxy aborts the downstream connection at the delivery time of the next event that is delivered, so the client sees a broken response, as a cut. That is with the head unwritten if the lost event was the first. If no later event is delivered, the connection stays silent.

The proxy writes the response head with the response's first delivered message, and frames the body as chunks, or with the upstream's `Content-Length` when the body is one message, never delimited by closing the connection, so that an abort always reaches the client as an error.

**EMU-45** **Upstream.** The proxy MUST open one upstream connection for each downstream connection, when that connection's first request is forwarded, and close it with it.
- **Time.** The connection's setup, including TLS, is part of the request's time at the server, after its delivery, not of the uplink.
- **Failure.** A refused, failed or reset upstream connection makes the proxy close the downstream connection at once, so the client sees a transport error. A request already offered keeps its fate.
- **After the client leaves.** A delivered request MUST be forwarded even when its client has already given up, and its response still crosses the downlink (EMU-33). It is recorded by no attempt, so forwarding runs apart from the downstream connection.
- **TLS.** An `https` endpoint needs TLS upstream, which is compiled only with the `real-api` feature (HAR-20). Without it, the proxy refuses an `https` endpoint when it starts.

**EMU-46** **Transparency.** Apart from timing and drops, the proxy MUST forward requests and responses unchanged: the method, query, headers, status and bytes. The exceptions:
- hop-by-hop headers (RFC 9110) are dropped;
- `Host` is set to the endpoint's;
- the endpoint's base path is prefixed to the request's path;
- the attempt header of EMU-47 is stripped.

The proxy MUST NOT log headers or bodies (HAR-22).

**EMU-47** **Records.** Through a proxy, the client MUST tag each attempt with a header `x-acn-attempt`: a decimal number unique within the replicate. The proxy MUST keep every message's fate, size and receive time by attempt, and every fate in the order it decided them. A request without the header is carried, recorded under no attempt.
- **Collecting.** The harness collects an attempt's records when the attempt ends, keeping those carried by its end as EMU-36 says, and emits the link spans and the scenario span's events from them as in `sim` (EMU-36, EMU-37).
- **Times.** Their times are on the run's clock: the replicate's origin plus the link's time. (In `sim`, every replicate's origin is 0.)
- **Without a proxy.** The header is never sent, and the HAR-23 probe always goes straight to the endpoint.

**EMU-48** **Twins.** With the same seed and scenario, a sim run and a live run MUST take the same draws per message index on each link (EMU-9). Where a link's parameters do not depend on time (no outage window, no trace) and both runs offer their messages in the same order, every message therefore takes the same loss decision, reorder selection and jitter. Two things are different, and are compared as distributions (CON-25, T11b):
- what depends on send times: rates, holds, outage windows, a trace's segment;
- the order of concurrent messages, including a timed-out attempt's late response next to its retry.

**EMU-49** **No scenario, no proxy.** A live run with no scenario MUST go straight to its endpoint, as before T12, and EMU-39 holds for it. The run's `opt.endpoint` and endpoint host name the real endpoint whether or not a proxy carries the calls, so a proxy's ephemeral port never enters `run_id` (CON-29).

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

### §2 and §3: link models and scenarios

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

### §4: the sim engine

- `crates/acn-emu/tests/sim_engine.rs` — EMU-30 to EMU-32, EMU-34, EMU-38, and the network's part of EMU-35 (T11.1):
  - the queue's order, groups and refusal of the past;
  - a request delivered later than sent, and requests sent at different times delivered at one instant (through a hold);
  - response messages of concurrent calls offered to the downlink in time order;
  - the order rule within a response;
  - each drop of EMU-35;
  - a response's send times refused when they decrease, a response refused for a request that was not delivered, and a refusal that registers nothing;
  - a golden vector of a network's events on `cellular-handover`, crossing both halves of the handover (an uplink hold and a downlink drop), with every fate and outcome;
  - two runs on `5g-iana-replay` giving identical results.
- `tests/accept/harness_sim.rs` (T11.2) — EMU-39: the bundles of four runs with no scenario, pinned, unchanged by the scheduler's move onto the queue.
- `crates/acn-harness/tests/network.rs` (T11.3) — EMU-33 to EMU-36:
  - one session with and without a zero-delay network gives the same exchanges, batches included (one marker, so one set of prompts);
  - over a delayed path every timestamp moves by the delays;
  - a request delivered after its timeout still reaches the mock;
  - an attempt keeps nothing received after its end, in its events, its bytes or its link records;
  - a wait and an attempt that end at one instant resume in the order they would without a network.
- `tests/accept/emu_sim.rs` (T11.3) — EMU-35 to EMU-37:
  - a scenario run twice byte-identical, and its run id not the no-scenario one;
  - link spans under their `chat`, from the one `acn-emu` resource, each linked to the one `acn.scenario` span, with TRC-15's attributes;
  - a lost request ending at its deadline, with its outage event;
  - a held request recording its hold and its window;
  - cut streams retried;
  - a trace run's step events, per direction, with their parameters, in time order, recorded once, with `link.parquet`'s step following each message's own direction;
  - a scenario with two paths, or in `live`, refused.
- Scenario runs in `sim` have no live twin until T12.3 (CON-25): until then their numbers are exploratory, not cited.

### §5: the live proxy

- `crates/acn-emu/tests/live_proxy.rs` (T12.2) — EMU-40 to EMU-46:
  - framing of requests, bodies and SSE events, including chunks that split or join events, `\r\n` endings and comment blocks;
  - send times in order under concurrent connections;
  - delivery never early, within a loose upper tolerance;
  - each drop of EMU-44 as the client sees it (a timeout, or a broken response, with the first event lost too);
  - an upstream refusing or resetting;
  - a request forwarded after its client left;
  - transparency of method, path, query, headers and bytes, with `Host` and the base path rewritten;
  - HTTP/2 and upgrades refused;
  - links built before the first connection, and everything closed at shutdown.
- The harness's live tests (T12.3) — EMU-47 to EMU-49:
  - link spans and scenario events from a live run, on the run's clock;
  - a lost request timing out, and a cut stream retried;
  - a live run with no scenario unchanged;
  - a sim run and a live run of one sequential session taking the same draws per message index.

### §6: measured traces

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
3. ~~How the live proxy cuts a byte stream into messages.~~ Settled by T12 (ADR-36): by HTTP message and SSE event, as in sim (EMU-41).
