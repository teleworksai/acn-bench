# ADR-32 — T10: link models and synthetic scenarios

**Status:** accepted (T10; `spec-change`, SPEC 020 Draft v0.1 §2 and §3; Class B, `acn-emu`). **IDs affected:** EMU-1 to EMU-9, EMU-20 to EMU-22; CON-5, CON-27(a), CON-30(b); TRC-15.

## Context
TASKS.md's T10 asks for:
- a `LinkModel` trait with delay and jitter, a rate limit with a token bucket, independent and Gilbert–Elliott loss, reordering and scheduled outages;
- a scenario loader;
- tests on the models' statistics.

SPEC 020 described none of these. The sim engine (T11) and the live proxy (T12) will both drive these models. So the models have to produce the same fate for the same seed whichever drives them, and on every machine (CON-5).

## Decision
- **Messages, not packets.**
  - A link carries application messages with a size.
  - Packet-level behaviour (segmentation, TCP's own reaction to loss) belongs to the live proxy and to netem, not to this model.
  - A message is lost whole. That is pessimistic for a large message, and it is what a sim of turns over a link needs.
- **One fixed stage order: outage, loss, rate, delay, reorder.** This is the order a message meets a path:
  - a radio gap first;
  - then loss on the medium;
  - then serialisation at the bottleneck;
  - then propagation and jitter;
  - then reordering.

  A message dropped early never reaches the bottleneck queue, so a lossy link does not also congest itself.
- **Exact integers everywhere.**
  - Probabilities are parts per million, decided by an exact rejection draw over `0..10⁶`, the same way the mock decides its faults (MLM).
  - The token bucket keeps credit in bit-nanoseconds in an `i128`, and rounds a departure up to the next nanosecond.
  - No floating point touches a fate, so a fate cannot depend on the platform's floating-point behaviour.
- **Jitter is uniform only.** A uniform integer draw is exact. A normal or Pareto jitter would need floating point or a table, and T10b's trace work may show which shape matters (lab item (d) asks the same). Adding a shape later is a new key, so no existing scenario changes.
- **The order rule.** Without reordering, a link is FIFO: a message's delivery is raised to the previous message's. That matches a single path whose jitter comes from queueing.
  - Reordering is explicit (EMU-8). A selected message gets the gap on top of its in-order time, so it always arrives after every earlier unselected message, and later messages may overtake it. So TRC-15's `reordered` means "selected and held back", never "arrived early by chance".
  - netem's behaviour, where jitter alone reorders, is not what most real paths do. A scenario that wants it asks for reordering.
- **The token bucket owes.**
  - A message larger than the bucket waits for a full bucket, then takes its whole size, leaving the credit negative.
  - So a long run of large messages still settles at the rate. The alternative, refusing messages larger than the bucket, would make the burst size a hidden message-size limit.
- **Tail drop at the queue limit.** The backlog counts accepted messages whose departure is later than now. The limit applies only to a message that would wait: one that leaves at once never joins the queue, whatever its size, so the queue limit is not a hidden message-size limit either. A drop takes no credit. This is the simplest queue discipline; AQM is out of scope until a POC needs it.
- **Per-stage sub-streams, drawn by message index (EMU-9).**
  - Each stage has its own sub-stream, `link.<name>.<direction>.<stage>`. Every stage present draws for every message offered to the link, before the pipeline runs, whether or not an earlier stage drops the message. The Gilbert–Elliott state moves for every message too.
  - So message k's draws in every stage are fixed by the seed and k alone. Two treatment cells that differ only in loss give every message both deliver the same jitter, and the effect of loss can be read on its own. A first draft drew only for messages that reached a stage, so changing loss shifted which message got which jitter; the review caught it.
  - The draws are the impairment schedule of CON-5(d): fixed before any traffic. The live proxy (T12) can precompute it, and the arithmetic of the bucket, the FIFO rule and outages is a pure function of it and the observed send times.
  - The draw algorithm is pinned in the spec (rejection sampling with `z = (2⁶⁴ − 1) − ((2⁶⁴ − 1) mod n)`), because another exact algorithm would give different fates.
- **Times and their range.** A link's times are nanoseconds from its origin, the start of the replicate, so an outage window at 10 s falls 10 s into every replicate, whatever runs before it. Every time and duration is bounded by 2⁶² ns (about 146 years), so sums of two never overflow, and anything beyond is refused with reason `range`.
- **Outages apply again after a hold.** A message held to a window's end meets an adjacent window, if there is one. The fate records the last window met, for TRC-34's `outage_id`, and the hold time, so that `deliver = send + hold + rate wait + delay`.
- **Scenario keys carry their unit** (`delay_us`, `start_ms`, `rate_kbps`). One unit is fixed per key, so a file cannot mean two things. The loader converts to nanoseconds and refuses overflow.
- **Loading and building are two steps.** `scenario::load` checks the file and returns its hash and link parameters; `Scenario::build(seed)` builds the links for one replicate. A scenario is loaded once per run and built once per replicate.
- **Measured traces are not wired yet.** `[link.trace]` exists in the format so that the key is reserved, but the loader refuses it (EMU-22). How a trace drives a link (replayed per sample, or fitted to these models) is SPEC 020 §10 question 1. It becomes a new task, T10b, after T10, so that this PR holds one spec concern.
- **Two synthetic scenarios.**
  - `clean` is the do-nothing baseline.
  - `cellular-handover` uses every stage, with guessed numbers, labelled as guessed in its comment, as ADR-29 allows.
  - `scenarios/synthetic/.gitkeep` is removed.

## Consequences
- The sim engine (T11) and the live proxy (T12) call `LinkModel::transmit` and record each `Fate` in TRC-15's `acn.link` span:
  - `send_ns` is the enqueue time;
  - the delivery time is the dequeue time;
  - `rate_wait_ns` is `acn.link.rate_limited_ms`;
  - `delay_ns` is `acn.link.applied_delay_ms`;
  - `reordered`;
  - a drop.
- `tests/link_models.rs` pins a golden vector of fates for one seed. Changing any stage's arithmetic or draws moves it, and every earlier sim run with it, so such a change is visible in review.
