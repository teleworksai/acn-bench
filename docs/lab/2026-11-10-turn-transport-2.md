# Lab note — turn-transport, second spike: framing against transport (2026-11-10)

Crate: `lab/turn-transport`. Track: lab (CON-23). **Every number here is exploratory and
may not be cited (CON-24).** Loopback, synthetic workload, stand-in model, one machine
(macOS arm64), wall-clock timers, median of 3 reps. Same workload and link as the first
note (`docs/lab/2026-09-21-turn-transport.md`), so the tables line up with it.

## Question

The first spike left two things open in its own findings:

1. Its downlink result compared two complete implementations: SSE and JSON over TCP
   against a raw stream over QUIC. How much of the difference is the framing, and how
   much the transport?
2. It guessed that coalescing tokens "would save most of the rest". Does it, and what
   does it cost?

## What was tried

Three arms were added to the five of the first spike. Nothing else changed.

| arm | transport | request | reply |
|---|---|---|---|
| `raw-tcp` | the baseline's TCP path through the same relay, no TLS | `len u32 \| records`: the whole context, every turn, as the canonical records the QUIC arms send | the output as raw bytes, ended by one NUL |
| `raw-tcp-coalesce` | as `raw-tcp` | as `raw-tcp` | the server writes at most once per 100 ms |
| `turn-coalesce` | as `turn` (quinn, delta prefix, resume) | as `turn` | the server writes at most once per 100 ms |

`raw-tcp` is the framing ablation. Against `sse` it has the same transport and the same
whole-context request without HTTP, JSON or SSE. Against `quic` it has the same payload
on the other transport. Like `sse`, it retries a dead connection from scratch.

Coalescing is the simplest possible: after the first new output, the server waits
100 ms and writes whatever has been generated. At 20 ms per token that is five tokens
a write. It also holds the first token back, which shows in the results below.

Run: `cargo run --release` in `lab/turn-transport`, defaults (6 turns; 16 KiB system
and tools, +2 KiB per turn; 100 tokens per turn at 20 ms; 150 ms prefill; 40 ms RTT,
10 Mbit/s up, 50 Mbit/s down; 2 s gap 1 s into turn 3). Every arm's output is checked
against what the model produced, byte for byte, across retries and resumes.

## What was learned

### The counters, first

The relay counts payload bytes at its ingress. For the QUIC arms that is every
datagram: handshake, ACKs, MTU probes. For the TCP arms it is only what the
application wrote: no TCP or IP headers, no ACKs. This was a stated bias of the first
note. It matters more here, because `raw-tcp` is the first arm whose application bytes
are small enough for the uncounted headers to dominate. Where a figure below adds
headers back, it is an estimate and says so.

### No gap

| arm | up B | down B | up pkts | down pkts | turn ms | warm TTFT ms |
|---|---|---|---|---|---|---|
| sse | 153280 | 78378 | 12 | 612 | 2195 | 216 |
| raw-tcp | 150019 | 3420 | 12 | 603 | 2195 | 216 |
| raw-tcp-coalesce | 150019 | 3420 | 12 | 120 | 2216 | 315 |
| quic | 171904 | 32333 | 429 | 675 | 2222 | 241 |
| turn | 47946 | 30894 | 343 | 631 | 2177 | 197 |
| turn-coalesce | 41597 | 15271 | 142 | 136 | 2238 | 297 |

"pkts" is writes seen at the relay for the TCP arms and datagrams for the QUIC arms.

1. **On the downlink, the baseline's application bytes are almost all framing.** The
   600 tokens are 3 420 B of text. `sse` writes 78 378 B to carry them, 23 times as
   much: 131 B per token for the SSE event and its JSON. `raw-tcp` writes the text and
   six NULs.
2. **On the uplink, framing is nothing.** `raw-tcp` sends 2% less than `sse` (150.0
   against 153.3 KB): the HTTP headers and JSON quoting. The uplink is the context sent
   again every turn, and only the delta removes it (`turn`: 47.9 KB). The first note's
   "the delta is the whole uplink story" holds with the framing taken away.
3. **Framing costs no time here.** `raw-tcp` and `sse` have the same turn time and the
   same TTFT to the millisecond. At 50 Mbit/s down, 131 B a token is not a load. It
   would be at a few kbit/s, which this link does not model.
4. **Per token, the transport's own overhead is larger than the token.** A token is
   about 6 B. `turn` sends about 49 B of datagram for each (30 894 B over 631
   datagrams), and that is before UDP and IP headers. The TCP arms pay the same kind of
   cost where the relay cannot see it: 603 segments with 40 to 52 B of TCP and IP
   header each is an estimated 24 to 31 KB, against 3.4 KB of text. So the first note's
   "QUIC arms send ~31 KB against the baseline's 78 KB" was framing against
   packetisation, as it suspected, and with headers counted `raw-tcp` and `turn` are
   in the same range. **One token per packet is the cost, on either transport.**
5. **Coalescing halves the QUIC downlink and takes a third of its uplink packets'
   bytes.** `turn-coalesce` sends 15.3 KB down against 30.9 KB, in 136 datagrams
   against 631. Its uplink falls from 47.9 to 41.6 KB, and from 343 datagrams to 142,
   because there are fewer packets to acknowledge: the first note's "the next uplink
   win is ACK frequency" is the same effect. Per turn after the first, `turn-coalesce`
   sends about 2.74 KB up, of which about 2.1 KB is the request.
6. **This coalescer costs 100 ms of TTFT, and it need not.** Warm TTFT goes from 197
   to 297 ms on `turn` and from 216 to 315 ms on `raw-tcp`, exactly the interval,
   because the first token waits like every other. Turn time rises by 20 to 60 ms. A
   coalescer that writes the first token at once and holds only the ones after it
   would keep the TTFT; it was not built.
7. **On TCP, coalescing changes the write count and not the counted bytes** (120
   writes against 603, the same 3 420 B). Its saving is in the headers the relay does
   not count, an estimated 19 to 25 KB of the 24 to 31 KB above.

### With a gap

| arm | 2 s blackout, gap turn ms | 2 s break, gap turn ms | up B with a break | output thrown away on break |
|---|---|---|---|---|
| sse | 3025 | 5240 | 180159 | 231 B |
| raw-tcp | 3025 | 5239 | 176328 | 231 B |
| raw-tcp-coalesce | 3023 | 5258 | 176328 | 206 B |
| quic | 3312 | 5297 | 207875 | 221 B |
| turn | 3265 | 3093 | 55341 | 0 |
| turn-coalesce | 3327 | 3092 | 49564 | 0 |

8. **Framing changes nothing about recovery.** `raw-tcp` behaves as `sse` does in both
   gaps: it stalls through a blackout and starts the turn again after a break. What
   recovers a break in 3.1 s is the turn outliving its stream (`turn`), which is the
   first note's finding and is unchanged by coalescing.
9. **Coalescing does not hurt resumption.** `turn-coalesce` resumes after a break in
   the same 3.09 s and throws nothing away. After a blackout it is 60 ms slower than
   `turn` (3 327 against 3 265 ms), about the hold interval.

The blackout sweep and the deadline and eviction checks of the first note were run
again with the new code and are within a few milliseconds of it; they are not repeated
here.

### What this does to the first note's claims

- **Narrower:** "downlink is framing, not tokens" was right about `sse` and wrong to
  credit the saving to the raw stream alone. The saving over `sse` is the framing
  (finding 1). What is left after it is one packet per token (finding 4), and that is
  what coalescing addresses (finding 5).
- **Unchanged:** the delta prefix for the uplink, and the turn that outlives its
  stream for a break. Neither depends on the framing or on coalescing.

### Known biases

Those of the first note all stand. In addition:

- **The TCP arms' packet counts are writes at the relay, not segments on a wire.** With
  `TCP_NODELAY` and a 20 ms cadence one write is one segment in practice; this was not
  captured.
- **The header estimates in findings 4 and 7 are arithmetic, not measurements**: 40 B
  for IPv4 and TCP without options, 52 B with timestamps, per write.
- **The QUIC figures exclude UDP and IP headers** (28 B a datagram over IPv4), so they
  understate QUIC on the wire as well, by about 19 KB down for `turn` and 4 KB for
  `turn-coalesce`.
- **One coalescing interval, one token cadence.** 100 ms against 20 ms. The trade
  between interval, TTFT and bytes was not swept.

## Graduate / park / drop

**Recommendation: park, as before, and carry three things into the SPEC 125 draft
when it is written.**

- The evaluation needs a **raw-stream-over-TCP control** beside the SSE baseline.
  Without it a turn-native transport is credited with what dropping SSE would give any
  transport.
- **Bytes have to be counted at one layer for every arm**, IP or below. The relay's
  payload counters cannot compare a TCP arm with a UDP arm once the application bytes
  are small. This is what `acn-emu`'s live proxy, or `netem`, would have to give before
  any of this graduates; it is the same blocker the first note named for its time
  results.
- **Token coalescing is a knob of its own**, independent of the transport, with TTFT
  as its cost. It belongs in the evaluation as a factor, with a first-token-immediate
  variant.

Open for a next spike: the first-token-immediate coalescer; a sweep of interval
against cadence; a low-rate link (tens of kbit/s) where finding 3 should stop holding;
and the first note's remaining list (authentication of `RESUME`, MTU and 0-RTT carried
across a resume, `RESUME` landing on another server instance).
