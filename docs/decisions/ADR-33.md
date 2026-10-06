# ADR-33 — T10b: a measured trace drives a link by replay

**Status:** accepted (T10b; `spec-change`, SPEC 020 Draft v0.2; Class B, `acn-emu`). **IDs affected:** EMU-10, EMU-11, EMU-12, EMU-22; EMU-9; CON-5(d), CON-27(a); TRC-15. **Settles:** SPEC 020 §10 question 1.

## Context
T08 brought in the 5G-IANA drive test as `scenarios/measured/5g-iana-2023-01-29/`. T10 left open how a trace drives a link, and reserved `[link.trace]`. The trace has 198 samples about 26 s apart, each with:
- a round-trip time (average, minimum, maximum, standard deviation);
- a loss fraction;
- a downlink and an uplink throughput.

Nineteen of the samples lost every probe. The two ways to use it were to replay it sample by sample, or to fit it to the §2 models.

## Decision
- **Replay, sample by sample.** Each sample's parameters are in force over its interval of trace time. The run then sees the trace's timeline: its slow throughput swings, its loss episodes and its outages, in their order. A fitted model would keep only the statistics. Replay is what CON-21's provenance and the "measured, not guessed" argument of PLAN.md §2 point to. Fitting stays a later option (§10 Q1).
- **The mapping.** Values are rounded from the trace's `f64` to integers once, at load, so a fate stays exact-integer arithmetic.
  - The one-way delay is half the RTT average. The jitter is uniform (EMU-3) with a half-range of `stdev × √3 / 2`, so each direction's jitter has half the RTT's standard deviation. Both are approximations: the trace measures round trips, not each direction, and the two directions' jitter add to the RTT's standard deviation only if they are fully correlated (with independent directions the emulated RTT's spread is `1/√2` of the trace's). The asymmetry of a real 5G path is not in the data, so it is not invented here.
  - Where the half-range exceeds the delay (sample 2: 26 ms of delay, a 33 ms half-range), the clamp at the departure time of EMU-3 cuts the low tail, which raises the mean delay a little. That is a property of a uniform shape on a short delay, kept rather than hidden.
  - A first draft used half the standard deviation as the half-range, understating the spread by √3; the review caught it.
  - The loss is independent, at the sample's fraction. Bursts within a 26 s sample are not in the data either.
  - The rate is the throughput measured in the link's direction.
  - A sample with total loss, or with zero rate in the direction, is an outage. It drops what is sent in it and accrues no credit. Total ping loss makes a sample an outage even when the throughput test measured something (sample 3: loss 1, 381 kbit/s up): the ping is the link-state probe, and a link that loses every probe is down for a turn's purposes.
  - An outage sample has no RTT, so it carries the delay and jitter of the nearest earlier sample with one. A message queued before an outage and leaving the rate stage just as it begins would otherwise arrive with no delay.
  - The bucket and queue sizes come from the scenario, because a throughput test measures neither.
- **The trace repeats.** Link time maps to trace time `(τ + start_s) mod period`, so a run longer than the 86-minute drive keeps going.
  - `start_s` is part of the scenario, so every replicate replays the same timeline and only the draws differ: replicate spread measures draw noise, not variation along the drive. Different scenarios (or a POC's grid over `start_s`) cover different moments. A seeded per-replicate offset is a later option.
  - Each sample is held over its whole interval, including the four long gaps of the drive (41 to 98 s). The last sample lasts `sample_interval_s` (26 s), although the drive's last samples are 13 to 14 s apart, so the closing outage is about 12 s longer than measured in each period. Both follow the trace format's own definitions (EMU-62) rather than guessing.
- **Time-varying stages generalise the static ones (EMU-10, EMU-11).**
  - The rate stage integrates credit exactly across segments, skipping whole periods arithmetically, so a long wait costs one pass over the segments, not one per period.
  - A static link is one segment, and its arithmetic is the same: the golden vector of T10 holds unchanged.
- **Draws stay by message index (EMU-9).**
  - **Loss:** the draw is EMU-9's over `0..10⁶`, compared with the segment's rate.
  - **Jitter:** the range depends on the segment, so on timing. A rejection draw would then use a timing-dependent number of generator outputs and break the alignment by index. So a trace link's jitter is one 64-bit output scaled by multiply-and-shift, with a bias below 2⁻²⁰. Static links keep the exact rejection draw.
- **A trace link has no other stages but reordering.** Mixing a trace's loss with a scenario's loss would mean two loss models at once, with no definition of how they combine. A scenario that wants extra impairment on top of a trace is a later decision.
- **`dir` is relative to the scenario file's directory** (or absolute, which the tests use for a scenario copied elsewhere), so the loader does not need to find the workspace root, and a kit that keeps `scenarios/` together works. The hash in `[link.trace]` is the trace's BLAKE3 (EMU-64), checked at load (CON-27(a)), so a scenario's hash commits to the trace it replays.
- **`scenarios/synthetic/5g-iana-replay.toml`** replays the trace on both directions. Its bucket and queue sizes are guessed, and its comment says so.

## Consequences
- The sim engine (T11) can run a POC over the measured 5G drive. TRC-15's outage events for trace outages take cause `trace`, and the fate's `sample` gives the trace sample in force at sending. T11 will need a public way to list a trace link's outage intervals in link time, to emit those events; `TraceSchedule::at` is private today.
- Because the trace's rate drops to a few kbit/s in places, a run can see queues that wait for minutes. That is the trace, not a fault. A POC that wants a gentler link chooses `start_s`.
