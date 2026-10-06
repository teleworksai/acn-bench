# SPEC 020 — Link emulation: models, engine, proxy and measured traces

**Status:** Draft v0.1 (October 2026). Partial: §2, link models, and §3, scenario files, are written for T10; §6, measured traces, for T08. The sim engine (§4, T11, numbered from 30) and the live proxy (§5, T12, numbered from 40) are to write. **Inherits:** SPEC 000, 010. **Prefix:** EMU. **Crate:** `acn-emu` (Class B); `scenarios/synthetic/` is Class A, and `scenarios/measured/` is in the frozen set (CON-7).
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
  - `[link.trace]`: `dir` and `blake3` (EMU-22).

  The loader converts every duration to nanoseconds.

**EMU-21** `acn_emu::scenario::load` MUST refuse a scenario that breaks EMU-20, with one of these reasons:
- `layout`: the file cannot be read, or is not a `.toml` file;
- `parse`: not TOML of the EMU-20 shape, including an unknown or missing key;
- `name`: a name that is not a name, or a `name` other than the file stem;
- `duplicate`: two links with one name and direction;
- `range`: a parameter out of its range, such as a probability above 10⁶, a zero rate, burst or queue, a zero reorder gap, an outage window that is empty, unsorted or overlapping, or a duration beyond 2⁶² ns;
- `trace`: EMU-22.

It MUST return the scenario's hash and its link parameters. `Scenario::build` MUST then build each link's model (EMU-1) under a given replicate seed.

**EMU-22** A link that names a measured trace (§6) does so in `[link.trace]`, by the trace's directory and the BLAKE3 of its `trace.toml` (CON-27(a)). How a trace drives a link is §10's open question 1. Until it is settled, the loader MUST refuse a scenario that names a trace, with reason `trace`. The task that settles it adds the hash check.

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

- `crates/acn-emu/tests/link_models.rs` — EMU-1 to EMU-9:
  - **The statistics of each stage**, over many messages and several seeds:
    - the loss rate of EMU-5 within four standard deviations of `loss_ppm`;
    - the stationary loss and mean burst length of EMU-6 against their closed forms;
    - jitter within its range, with the mean of the uniform;
    - throughput settling at `rate_bps`, a burst within the bucket leaving at once, and tail drop at the queue limit;
    - the reorder fraction against `reorder_ppm`.
  - **The structure**: FIFO delivery without reorder, outage `drop` and `hold`, the stage order of EMU-2, the refusal of a message sent out of order, the index-aligned schedule of EMU-9 (changing the loss rate leaves every delivered message's jitter unchanged, and changing the jitter leaves the losses unchanged), each stage's edge cases (the bucket cap after idle, the backlog boundary, a tail drop taking no credit, a message larger than the queue leaving at once, adjacent windows, the reordered message's exemption, the clamp at `t`), the exact draw algorithm, and refusals of out-of-range times, and a golden vector of fates for one seed.
- `crates/acn-emu/tests/scenario.rs` — EMU-20 to EMU-22: every scenario under `scenarios/synthetic/` loads and reads as written, field by field; each malformed variant is refused by name; a scenario naming a trace is refused.

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

1. How a scenario uses a measured trace: replayed sample by sample (each sample's loss, one-way delay and rate in force for its interval), or fitted to the models of §2. T10 leaves it open (ADR-32); T10b settles it and lifts EMU-22's refusal.
2. Whether `scenarios/measured/` should also hold our own captures (T08's original plan, a phone-tethered walk) in the same format, with `mobility = "walk"`. That is expected; the format does not depend on the source.
3. How the live proxy (§5, T12) cuts a byte stream into the messages of §2: by HTTP message, by streamed chunk, or by write. TRC-15 allows a message or a byte segment.
