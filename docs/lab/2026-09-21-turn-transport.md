# Lab note — turn-transport (2026-09-21)

Crate: `lab/turn-transport`. Track: lab (CON-23). **Every number here is exploratory and
may not be cited (CON-24).** Loopback, synthetic workload, stand-in model, one machine
(macOS), wall-clock timers, median of 3 reps.

This note was corrected after a pre-merge review (Claude adversarial, Claude
maintainability, Codex adversarial). What the review overturned is listed at the end,
because the first version's most striking result was an artefact.

## Question

How much of POC 11 (prefix redundancy), POC 12 (stream survivability) and POC 14
(protocol chattiness) does a turn-native transport collapse into one mechanism?

## What was tried

One binary, five arms, all over the same impaired loopback link:

| arm | transport | prefix | when the path breaks |
|---|---|---|---|
| `sse` | HTTP/1.1 (hyper), keep-alive, SSE events shaped like a messages API, no TLS | whole conversation as JSON, every turn | reconnect, re-send, server generates again |
| `quic` | quinn, one bidi stream per turn | whole context, every turn | reconnect, open the turn again |
| `quic-migrate` | as `quic` | as `quic` | same connection, `Endpoint::rebind`, QUIC path migration |
| `turn` | as `quic`, plus the turn protocol below | delta against what the server holds | new connection, `RESUME` at the byte offset received |
| `turn-probe` | as `turn` | as `turn` | as `turn`, and after 250 ms of silence mid-output: `RESUME` on a new stream of the *same* connection |

`quic` and `quic-migrate` are ablations: they show what QUIC does alone, so the `turn`
rows can be read as what the turn-level mechanism adds. quinn runs with its defaults
(no keep-alive, MTU discovery on) and a 10 s idle timeout.

**The turn protocol** (`src/turn.rs`). Client opens a stream and sends
`OPEN | flags | session | turn | deadline_ms | base_len | base_hash | delta`, 62 bytes
plus the delta. `base_len`/`base_hash` (blake3 over canonical records) name the context
the client believes the server holds; a server that holds something else answers
`NEED_FULL` and the client re-opens with the full context. The reply is one status byte
and then the output as the raw stream: no per-token framing, the byte offset *is* the
resume cursor. With the resumable flag set, the turn, not the stream or connection, owns
the generation, so it keeps running through a gap; `RESUME | session | turn | offset`
(21 bytes) re-attaches from any connection. `deadline_ms` is the budget left when the
request is sent; it bounds generation, every write, and the lifetime of the resumable
state. There is no authentication of any kind.

**The link** (`src/link.rs`). A relay every arm dials (TCP for `sse`, UDP for the QUIC
arms): 20 ms each way, 10 Mbit/s up, 50 Mbit/s down, unbounded queue, byte counters at
ingress. Two kinds of gap, opening 1.0 s into turn 3 (mid-generation), 2 s long in the
matrix:

- *blackout*: path survives, carries nothing. UDP datagrams are dropped; TCP bytes are
  held and serialised from the end of the gap (user space cannot drop TCP segments).
- *break*: as blackout, and every pre-gap flow is dead afterwards (handover with address
  change). TCP connections are closed, old UDP flows black-holed. Both clients get the
  same link-down/link-up signal an OS would give, and reconnect the instant the link is up.

Blackout recovery moves in steps, so the blackout is also swept from 1 s to 4 s.

**Workload.** 6 turns; 16 KiB system+tools, +2 KiB user/tool-result per turn, 100 tokens
per turn at 20 ms, 150 ms prefill in every arm (a warm provider prefix cache is assumed,
so re-sending the prefix costs bytes, not prefill). Every arm verifies the received output
against what the model produced, byte for byte, including across resumes.

Run: `cargo run --release --manifest-path lab/turn-transport/Cargo.toml` (about 110 s).
`--arm` or `--gap` narrows the matrix and skips the sweep and the checks; `--no-mtud`,
`--keep-alive-ms`, `--sweep-gaps-ms`, `--json` and the workload/link knobs are flags.

## What was learned

### Bytes (no gap)

Uplink bytes per turn:

| turn | sse | quic | turn | turn, session evicted before turn 3 |
|---|---|---|---|---|
| 0 | 18857 | 28839 | 28868 | 28844 |
| 1 | 21538 | 23229 | 3813 | 3842 |
| 2 | 24200 | 25905 | 3811 | 3843 |
| 3 | 26879 | 28620 | 3842 | 30845 |
| 4 | 29558 | 31309 | 3783 | 3808 |
| 5 | 32248 | 34013 | 3808 | 3812 |
| all | 153280 | 171944 | 47957 | 74985 |

1. **The delta is the whole uplink story.** `turn` is flat at ~3.8 KB per turn while both
   full-prefix arms grow by ~2.7 KB per turn; over 6 turns it is 69% less uplink than
   `sse`, and the gap widens with every turn (O(n) against O(n²)). QUIC alone (`quic`)
   costs *more* uplink than `sse`, not less.
2. **Of `turn`'s 3.8 KB, only ~2.1 KB is the request.** The rest is ~50 ACK packets for the
   ~100 one-token packets coming down. The baseline's TCP ACKs are not counted at all (see
   biases), so the 69% is conservative, but it also says the next uplink win is ACK
   frequency, not the header.
3. **A new QUIC connection costs ~8 KB up with quinn's defaults**, of which ~5.7 KB is
   path-MTU-discovery probes (`--no-mtud`: turn 0 drops from 28.9 KB to 23.2 KB, and the
   cost of reconnect + `RESUME` after a break from 7.5 KB to 1.7 KB). This swamps the
   21-byte `RESUME`. Anything that resumes by reconnecting should carry the MTU over.
4. **Downlink is framing, not tokens.** 600 tokens are ~3.4 KB of text. `sse` sends
   78.4 KB (131 B per token: the SSE event and its JSON); the QUIC arms send ~31 KB
   (~42 B per token without MTU probes: one short-header packet per token). That is a
   difference between two complete implementations, not a QUIC effect: there is no arm
   with a raw stream over TCP, which would shed the same SSE/JSON framing, and the
   baseline's TCP/IP headers are not counted. Coalescing tokens would save most of the rest.
5. **Eviction costs one round trip and one full prefix**: turn 3 took one `NEED_FULL`
   fallback, went to 30.8 KB and TTFT from 197 ms to 295 ms. That is the link to POC 7
   (re-homing).

### Time

| arm | no gap, turn ms | 2 s blackout, gap turn ms | 2 s break, gap turn ms | warm TTFT ms | output thrown away on break |
|---|---|---|---|---|---|
| sse | 2195 | 3023 | 5239 | 216 | 231 B |
| quic | 2224 | 3330 | 5304 | 244 | 221 B |
| quic-migrate | 2223 | 3339 | 3091 | 244 | 0 |
| turn | 2177 | 3293 | 3096 | 197 | 0 |
| turn-probe | 2178 | 3054 | 3096 | 197 | 0 |

Uplink for the whole conversation with a break: `sse` 180.2 KB, `quic` 208.0 KB,
`quic-migrate` 172.3 KB, `turn` 55.4 KB.

Blackout length sweep, gap turn ms (silence probes sent):

| blackout ms | sse | turn | turn-probe |
|---|---|---|---|
| 1000 | 2197 | 2180 | 2179 (4) |
| 1500 | 2525 | 2733 | 2552 (6) |
| 2000 | 3023 | 3311 | 3053 (8) |
| 2100 | 3123 | 3313 | 3305 (9) |
| 2500 | 3524 | 4477 | 3556 (10) |
| 3000 | 4024 | 4459 | 4057 (12) |
| 4000 | 5024 | 6750 | 5065 (16) |

6. **Break: resume turns a 5.2 s turn into a 3.1 s one**, the floor being gap end plus a
   handshake. The generation ran through the gap, so everything was waiting when the client
   came back; `sse` and `quic` throw away what they had and pay for the turn twice.
7. **But QUIC migration alone does exactly as well (3091 ms vs 3096 ms)** when the server
   accepts it. Turn-level resume only earns its keep where migration is unavailable: load
   balancers that do not route on connection ID, a re-homed or restarted server, an idle
   timeout, a client that lost its process. None of those was tested here. That is the
   honest size of the POC 12 contribution: a fallback below migration, not a replacement.
8. **Blackout: QUIC recovers in steps, and the steps double.** After the link returns,
   nothing moves until the server's next probe timeout, and those back off exponentially:
   the `turn` column sits at ~2.7 s, ~3.3 s, ~4.5 s, ~6.8 s as the blackout grows. At 2 s
   that is 0.3 s behind the baseline, at 4 s it is 1.7 s behind. The baseline column is
   flattered (held bytes released at gap end, no RTO backoff; kernel TCP backs off in
   doubling steps too, and would land near 4 s for the 2 s blackout by RTO arithmetic,
   not measured), so whether QUIC is ahead of or behind real TCP here is not known.
9. **The silence probe removes the steps.** `turn-probe` knows tokens arrive every 20 ms,
   treats 250 ms of silence as trouble and sends `RESUME` on a new stream, which QUIC
   transmits at once where the stalled stream waits for a timer. It tracks the baseline to
   within 30-180 ms across the sweep (the spread is where in its 250 ms interval the gap
   ends: 3053 ms at 2000, 3305 ms at 2100) for ~0.9 KB of extra uplink at 2 s. Its gain
   over `turn` is 0 to 0.25 s up to 2.1 s, 0.9 s at 2.5 s, 1.7 s at 4 s. This is the one
   place where knowing the *turn's* cadence beat the transport's own recovery. Limits
   found in review: the silence interval must exceed a few round trips or every reply is
   abandoned before it arrives (now floored at three smoothed RTTs); each probe abandons a
   stream, so ~25 s of blackout would exhaust quinn's default 100 streams. Untested:
   whether a bare `rebind` on silence would do the same without any turn protocol.
10. **Warm TTFT: `turn` is 19 ms better than `sse`, `quic` 28 ms worse.** The delta removes
    ~25 KB of uplink serialisation at 10 Mbit/s; full-prefix QUIC pays congestion-window
    pacing that the baseline escapes (see biases). Cold turn 0 is ~60 ms worse on QUIC.
11. **Deadline: the mechanism does what it says, late by the request's upload time.**
    With a deadline at half the generation time (1075 ms), the server stopped generating
    at 248 of 570 B, told the client, which failed at 1178 ms, and held no turn state
    100 ms later. The ~100 ms lateness is a cold 18 KB full-prefix upload through slow
    start; the budget is relative because the clocks are not synchronised. With a deadline
    inside a break (2500 ms, generation already finished) the client gave up at its
    backstop, 2753 ms, and the server had dropped the resumable state. Nothing here
    measures a *benefit* from declaring a deadline, beyond bounding how long state lives.

### So: how much collapses?

- **POC 11 — yes.** Prefix redundancy is fully addressed by one field pair
  (`base_len`, `base_hash`) and server-held state. Nothing about it needs QUIC.
- **POC 12 — partly.** One mechanism (turn outlives stream, offset is the cursor) covers
  the break, where it ties with QUIC migration wherever migration is available, and, with
  the silence probe, removes QUIC's stepwise recovery from a blackout. How much that is
  worth against real TCP is unmeasured.
- **POC 14 — hardly.** A warm turn is one request and one response stream in every arm;
  the turn stream removes no round trip that keep-alive HTTP had. What it removes is
  per-token framing (finding 4). If POC 14 is about agent-protocol round trips (MCP
  initialise/list/call), this spike says nothing about it.
- The common mechanism is **server-held turn state named by a hash and resumed by an
  offset**, not the transport. From memory and unverified: at least one provider already
  offers both halves over HTTP (a previous-response id for the prefix, resumable background
  streams with a sequence cursor). What QUIC itself contributed was migration and
  independent streams, which the silence probe depends on.

### Known biases

Favouring the baseline:

- The relay terminates TCP, so the baseline's congestion control and loss recovery see
  loopback: no slow start, no RTO backoff, blackout modelled as a stall. QUIC's see the
  real impaired link. The TCP handshake is charged as one modelled RTT.
- Counters are payload at the relay: all of QUIC (handshake, ACKs, MTU probes) but none of
  TCP/IP's headers, SYNs or ACKs. Loss is only recorded for UDP.
- The baseline has no TLS; QUIC's TLS 1.3 handshake is on the wire and in the numbers.

Favouring the prototype:

- The baseline sends ~330 B of HTTP headers per request, a credential-length header
  among them, plus model parameters in the body; `OPEN` carries no credential and no
  parameters. About 1% of the 69%, but ~15% of `turn`'s 2.1 KB request.
- No request compression in the baseline, and no HTTP/2 or HTTP/3 arm.

Neither way, or unknown:

- Both clients learn of link-up instantly; real retry policies back off.
- Not modelled at all: random loss, jitter, bandwidth variation, queue drops, 0-RTT,
  concurrent turns on one connection (where QUIC's lack of head-of-line blocking would
  show), real prefill cost of a re-sent prefix on a cold cache.
- A client keep-alive of exactly 2000 ms (`--keep-alive-ms 2000`) delays blackout
  recovery on every QUIC arm without the probe to ~5.1 s; 0, 1000 and 5000 ms do not.
  The mechanism in quinn was not identified.

## Graduate / park / drop

**Recommendation: park until the L-M1 graduation review, then graduate a narrower claim.**

- The time results cannot be defended while TCP and QUIC are impaired by different means.
  Graduation needs packet-level impairment for both (acn-emu `live`, then `netem`), which
  does not exist yet. The byte results do not have this problem.
- What to carry into a SPEC 125 draft: (a) delta prefix with hash-named base and
  `NEED_FULL` fallback, as POC 11's treatment arm; (b) turn-outlives-stream with
  offset resume, evaluated specifically where migration is unavailable; (c) the
  cadence-driven silence probe, swept over blackout length, with `quic-migrate`, a
  bare-rebind variant and real TCP as controls.
- POC 14 should stay a separate experiment; this prototype does not subsume it.
- Open questions for the next spike: authentication of `RESUME` and `OPEN` (a capability
  per turn); ACK frequency and token coalescing on the downlink; a raw-stream-over-TCP arm
  to separate framing from transport; carrying MTU and 0-RTT tickets across a resume;
  silence threshold against jitter (false resumes; none occurred here); `RESUME` landing
  on a different server instance; the keep-alive interaction above.

## What the pre-merge review overturned

The first version of this note reported 5.0 s for every QUIC arm in the 2 s blackout,
called it quinn's PTO backoff, and credited the silence probe with a 2 s win. The 5.0 s
came from a 2 s client keep-alive the spike had set itself; with quinn's default it is
3.3 s, and the probe's win at 2 s is 0.25 s. The probe's 3.07 s was also a lucky phase
(2000 ms is eight silence intervals), hence the sweep. The deadline check never
exercised expiry: it used a deadline later than the end of generation and looked at the
server only after the gap. The per-token packet saving was credited to QUIC without a
control. In the code: queued UDP datagrams crossed the gap, held TCP bytes were released
unshaped, a retry re-declared the full deadline, the `DEADLINE` status was discarded by
the reset that followed it, an `OPEN` with an empty base wiped the session before its
hash was checked, an impossible resume offset was answered `OK`, a non-resumable abort
left its turn behind, and client errors were all counted as "path lost". All fixed; none
moved a byte figure, and of the time figures only the blackout column changed.
