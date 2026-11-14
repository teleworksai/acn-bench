# Lab note — turn-transport, third spike: the coalescer's knobs and a slow link (2026-11-13)

Crate: `lab/turn-transport`. Track: lab (CON-23). **Every number here is exploratory and
may not be cited (CON-24).** Loopback, synthetic workload, stand-in model, one machine
(macOS arm64), wall-clock timers, median of 3 reps. Same workload and link as the first
two notes (`docs/lab/2026-09-21-turn-transport.md`,
`docs/lab/2026-11-10-turn-transport-2.md`) unless a section says otherwise.

## Question

The second note left three things open about token coalescing and the link:

1. Its coalescer cost exactly its interval in time to first token (TTFT), "and it need
   not". Does a coalescer that writes the first token at once keep the TTFT, and what
   does it give up?
2. It used one interval (100 ms) at one token cadence (20 ms). How do bytes, packets
   and time move with the interval, and with the cadence?
3. It found that framing costs no time at 50 Mbit/s down, and said that would stop
   holding at a low rate. Where does it stop, and for which arms?

## What was tried

Two arms and two options were added. Nothing else changed.

| addition | what it is |
|---|---|
| `raw-tcp-coalesce-first` | `raw-tcp-coalesce`, with the first token of a turn written as soon as it exists. The fixed windows start at the second token. |
| `turn-coalesce-first` | `turn-coalesce`, with the first new output of a stream written at once and only later writes held back. (What a resumed stream owes the client already went out at once in the parent arm.) |
| `--coalesce-sweep-ms a,b,…` | runs the four coalescing arms, no gap, at each interval |
| `--wire-overhead` | the link charges 28 B for each UDP datagram and 40 B for each TCP relay read, on top of the payload, when it works out how long a unit occupies the link. The byte columns still count payload only. |

`--wire-overhead` exists because of the second note's counter bias. The link shapes
what the relay sees: whole datagrams for QUIC, application bytes for TCP. At 50 Mbit/s
that does not matter. At tens of kbit/s it would let the TCP arms send their headers
for free. The charge is arithmetic, one header per relay read, not a capture.

Runs, all `cargo run --release` in `lab/turn-transport`:

- defaults with `--coalesce-sweep-ms 20,50,100,200,400` (the full matrix of the first
  two notes, plus the sweep);
- `--gap none --token-ms 5` with a sweep over 20, 50, 100 and 200 ms, and
  `--token-ms 50` with a sweep over 50, 100, 200 and 400 ms: the two cadences were
  not swept over the same intervals;
- `--gap none --down-mbps 0.256`, `0.064` and `0.032`, each with and without
  `--wire-overhead`;
- `--gap none --down-mbps 0.064 --up-mbps 0.128 --wire-overhead`, once;
- the interval sweep again at 64 kbit/s down with headers charged. It moved nothing
  but TTFT, by about 9 ms, and is not tabulated.

Every arm's output is checked against what the model produced, byte for byte. No run
reported a failure.

## What was learned

### The first token at once

Default workload and link, no gap. "Turn ms" and TTFT are medians over the warm turns.

| arm | up B | down B | down pkts | turn ms | warm TTFT ms |
|---|---|---|---|---|---|
| raw-tcp | 150019 | 3420 | 603 | 2195 | 216 |
| raw-tcp-coalesce | 150019 | 3420 | 120 | 2216 | 315 |
| raw-tcp-coalesce-first | 150019 | 3420 | 126 | 2236 | 216 |
| turn | 47943 | 30880 | 630 | 2176 | 196 |
| turn-coalesce | 41539 | 15126 | 132 | 2199 | 297 |
| turn-coalesce-first | 41680 | 15292 | 137 | 2218 | 196 |

1. **Writing the first token at once gives the whole TTFT back.** `turn-coalesce-first`
   has `turn`'s 196 ms and `raw-tcp-coalesce-first` has `raw-tcp`'s 216 ms, at every
   interval tried (finding 6).
2. **It costs at most one write a turn.** `turn-coalesce-first` sends 137 datagrams down
   against 132, and 15 292 B against 15 126 B, about 1% more. On TCP it is 126 writes
   against 120. At some intervals of the sweep it costs none. The second note's saving stands: half of `turn`'s downlink bytes and
   about a fifth of its datagrams.
3. **The turn ends about 20 ms later on TCP, and it is the window's phase, not a
   cost of the idea.** The windows of `raw-tcp-coalesce-first` start one token later,
   so the last write does too (2 236 against 2 216 ms). A coalescer that also writes
   at once when the turn ends would remove the tail on both variants; it was not
   built. The same 20 ms on the `turn` arms (2 218 against 2 199 ms) is not a
   finding: the sweep's rows for the same configuration in the same run read
   2 227 and 2 217 ms, so these arms move by about that much between conversations.

With a gap the new arms behave as their parents do. After a break both `turn`
coalescing arms resume in 3.09 s and throw nothing away. After a 2 s blackout
`turn-coalesce-first` took 3 198 ms and `turn-coalesce` 3 517 ms, against 3 276 ms for
`turn`. Recovery after a blackout moves in steps (first note), these are single
medians of three, and the order between them is not a finding.

### The interval

`turn` arms, default cadence (20 ms a token, 100 tokens a turn), no gap. The first row
is `turn`, which does not coalesce.

| interval ms | down B | down pkts | up B | up pkts | TTFT ms, plain | TTFT ms, first at once |
|---|---|---|---|---|---|---|
| none | 30880 | 630 | 47943 | 343 | 196 | — |
| 20 | 22020 | 344 | 47653 | 326 | 217 | 197 |
| 50 | 18299 | 234 | 44871 | 241 | 248 | 196 |
| 100 | 15159 | 133 | 41532 | 140 | 297 | 196 |
| 200 | 13663 | 90 | 40113 | 97 | 397 | 196 |
| 400 | 12481 | 60 | 39131 | 67 | 596 | 196 |

Bytes and packets are from `turn-coalesce`; `turn-coalesce-first` is within 1.5% on
bytes and within 6 datagrams.

4. **Most of the saving comes early.** A 20 ms hold already takes 29% off the downlink
   bytes and 45% off the datagrams, because this coalescer's period is the interval
   plus the wait for the next token: about 40 ms, two tokens a write. 100 ms takes
   51% of the bytes. Going on to 400 ms adds 9 points for four times the hold.
5. **Much of what is left is not the tokens.** At 400 ms the conversation still
   sends 12.5 KB down for 3.4 KB of text, and the curve is still falling slowly.
   No run took the remainder apart. It holds the first turn's handshake, path-MTU
   probes, datagrams that carry only acknowledgements, and per-write overhead; the
   byte totals here are six-turn totals with one start-up in them.
6. **Without the first-token rule, TTFT rises by the interval, to within 2 ms** (217, 248,
   297, 397, 596 ms against 196). With it, TTFT does not move.
7. **The plain interval's cost that was not measured is the wait between writes.** By
   construction the client sees nothing for the interval plus a token gap, then
   several tokens together. No arm records the longest silence between reads, so
   this note cannot say what a 400 ms hold looks like to a reader, only that the
   turn's end moves by up to one interval: 815 ms for `raw-tcp-coalesce` at a 200 ms
   hold on a 5 ms cadence, against 710 ms for `raw-tcp`.

### The cadence

`turn-coalesce`, no gap, 100 tokens a turn. "Tokens a write" is 600 tokens over the
datagram count less 30: `turn` sends 630 datagrams for 600 tokens at every cadence,
so about 30 carry no token. It is an estimate, and agrees with the coalescer's
period of the interval plus one token gap.

| token ms | interval ms | down pkts | tokens a write | down B | up B |
|---|---|---|---|---|---|
| 5 | none | 630 | 1.0 | 30162 | 47961 |
| 5 | 20 | 152 | 5 | 15315 | 40517 |
| 5 | 100 | 59 | 20 | 12394 | 39132 |
| 20 | none | 630 | 1.0 | 30880 | 47943 |
| 20 | 100 | 133 | 6 | 15159 | 41532 |
| 50 | none | 630 | 1.0 | 31907 | 57941 |
| 50 | 100 | 239 | 3 | 19184 | 45003 |
| 50 | 400 | 101 | 8.5 | 14099 | 40520 |

8. **What matters is the interval measured in tokens.** A 100 ms hold is about
   twenty tokens a write at 5 ms a token and about three at 50 ms. Two settings
   with about twenty tokens a write, 100 ms at 5 ms a token and 400 ms at 20 ms a
   token, send 12 394 B and 12 481 B down. The same 15 KB downlink needs a
   20 ms hold at the fast cadence and more than 200 ms at the slow one. An interval
   given in milliseconds is a different treatment for each model speed.
9. **A slow cadence costs uplink, and coalescing gets it back.** At 50 ms a token
   `turn` sends 57.9 KB up against 47.9 KB at 20 ms, in 637 datagrams against 343,
   which is consistent with each token's datagram being acknowledged on its own;
   acknowledgements were not counted separately. With a 100 ms hold the
   uplink is 45.0 KB. This is the second note's ACK effect, and it is larger for
   slow models.

### A slow downlink

No gap, 10 Mbit/s up, median turn time over the warm turns, ms. "Charged" is
`--wire-overhead`.

| arm | 50 Mbit/s | 256 kbit/s | 64 kbit/s | 64 kbit/s, charged | 32 kbit/s | 32 kbit/s, charged |
|---|---|---|---|---|---|---|
| sse | 2195 | 2202 | 2219 | 2307 | 3400 | 4401 |
| raw-tcp | 2195 | 2195 | 2196 | 2204 | 2197 | 2214 |
| raw-tcp-coalesce-first | 2236 | 2237 | 2238 | 2244 | 2240 | 2251 |
| quic | 2222 | 2224 | 2231 | 2240 | 2240 | 2253 |
| turn | 2176 | 2177 | 2181 | 2184 | 2187 | 2193 |
| turn-coalesce-first | 2218 | 2222 | 2217 | 2229 | 2215 | 2231 |

10. **The second note's finding 3 stops holding at tens of kbit/s, and only for
    `sse`: between 64 and 32 kbit/s counting payload, between 256 and 64 with
    headers charged.** At 50 tokens a second its 131 B a token is 52 kbit/s of payload, 68
    with a 40 B header on each write. At 64 kbit/s it just falls behind when headers
    are charged (2 307 ms). At 32 kbit/s a turn takes 3.4 s, or 4.4 s with headers,
    against 2.2 s of generation: the link, not the model, sets the turn's length.
    The figures match the arithmetic (100 tokens × 171 B at 32 kbit/s is 4.3 s).
    The charged figures are an upper bound: the relay charges one header per write
    however far behind the link is, where a real TCP sender with a backlog fills its
    segments. So a turn at 32 kbit/s is between 3.4 and 4.4 s, and the 64 kbit/s
    result with headers depends entirely on that charge.
11. **Every arm without the framing still fits at 32 kbit/s on a warm turn.** One token per packet
    is about 46 B on TCP and about 66 B on QUIC with headers charged, 18 and 26
    kbit/s at this cadence. So at this rate the framing is the difference between a
    stream that keeps up and one that does not, and coalescing is not yet needed once the
    conversation is under way (the first turn is finding 13).
    A faster model or a slower link would reach the point where it is; that point was
    not run.
12. **Warm TTFT barely moves** (216 to 227 ms for `raw-tcp`, 196 to 212 for `turn`, at
    32 kbit/s charged): the first token is small and the queue is empty when it
    leaves.
13. **The first turn of the QUIC arms pays for its start on a slow downlink.** Turn 0
    takes 2 279 ms at 50 Mbit/s and 3 736 ms at 32 kbit/s charged for `turn`
    (3 044 ms for `turn-coalesce-first`), while `raw-tcp`'s first turn does not
    move (2 234 to 2 248 ms). `sse`'s does (4 437 ms), as its warm turns do. The likely cause is the handshake and quinn's path-MTU
    probes queued ahead of the first tokens; it was not confirmed by a run with
    `--no-mtud`.

### A slow uplink as well

128 kbit/s up, 64 kbit/s down, headers charged, no gap.

| arm | up B | warm turn ms | warm TTFT ms |
|---|---|---|---|
| sse | 153280 | 3972 | 1900 |
| raw-tcp | 150019 | 3832 | 1850 |
| quic | 172081 | 3904 | 1923 |
| turn | 48085 | 2325 | 346 |
| turn-coalesce-first | 41943 | 2349 | 343 |

14. **On a slow uplink the delta is the result, and it is a TTFT result.** Every arm
    that sends the whole context waits 1.85 to 1.92 s for its first token at the
    median warm turn, because the request, 21 to 32 KB over the warm turns, has to
    cross 128 kbit/s first. `turn`'s request is about 2.1 KB by the protocol's
    arithmetic, and it has its first token in 346 ms. The first note measured the
    delta in bytes; here it is about 1.5 s at the median turn, and it grows with
    the context: by the same arithmetic about 1.2 s at turn 1 and 1.8 s at turn 5.
    Per-turn TTFT was not printed. Removing the framing does nothing for it (`raw-tcp`
    1 850 ms against `sse` 1 900 ms).

### What this does to the earlier notes' claims

- **Confirmed:** the second note's finding 6, that the coalescer's TTFT cost "need
  not" be paid (findings 1 and 2).
- **Revised:** its finding 3 said framing would be a load "at a few kbit/s". For
  this workload it is one at 52 to 68 kbit/s (finding 10).
- **Sharpened:** "coalescing is a knob of its own" is two knobs: the interval in
  tokens, not milliseconds (finding 8), and whether the first token waits.
- **Added:** the delta's effect on TTFT when the uplink is slow (finding 14), which
  no earlier run could show at 10 Mbit/s up.
- **Unchanged:** the turn that outlives its stream, for a break.

### Known biases

Those of the first two notes all stand. In addition:

- **The header charge is arithmetic.** One 40 B header per relay read on TCP and 28 B
  per datagram on QUIC. TCP ACKs are still charged nowhere. On the TCP uplink a read
  is up to 16 KiB, so the uplink is charged about two headers a request where a
  wire would carry a header per segment; the whole-context arms' times in finding 14
  are slightly low for that reason.
- **The header charge does not shrink under a backlog** (finding 10), so the
  charged times of an arm that has fallen behind are upper bounds.
- **Warm medians are taken over turns whose context grows.** On a slow uplink
  that is the middle of a ramp (finding 14), not a level.
- **Byte totals are six-turn totals with one start-up in them.** No warm-turn
  downlink figure is given here, unlike the second note.
- **The `turn` coalescing arms' turn time moves by about 20 ms between
  conversations** of the same configuration (finding 3).
- **The slow-uplink run changed three things at once** (uplink rate, downlink
  rate, the header charge) and was run once.
- **The slow link queues without bound and drops nothing.** A real link at this rate
  has a finite buffer. quinn's congestion control and TCP's would both react to loss
  that this relay never produces.
- **The link's rate is the only thing that changed.** Round-trip time stayed at 40
  ms. A link at tens of kbit/s usually has a longer one.
- **The longest wait between reads was not measured** (finding 7).
- **Two coalescers that are not the same code**, as in the second note: fixed windows
  on TCP, a hold after new output on QUIC. Their periods differ at the same interval.

## Graduate / park / drop

**Recommendation: park, as before.** Three more things for the SPEC 125 draft, beside
the three the second note carried:

- **The coalescing factor is given in tokens a write**, or as an interval together
  with the cadence, and the first-token-immediate variant is the one to evaluate.
  The plain variant only adds a known TTFT cost.
- **The evaluation grid needs a slow point** on each direction: a downlink where the
  baseline's framing no longer keeps up, and an uplink where the whole-context
  request is seconds long. Both effects are invisible at the rates used so far.
- **Bytes and link time have to be accounted at one layer for every arm.**
  `--wire-overhead` is a stand-in for that and should not survive into a result.
  This is still the blocker the first two notes named.

Open for a next spike: the longest silence a reader sees under each interval; a write
at the end of the turn so the tail does not wait; a finite buffer on the slow link;
whether path-MTU probes explain the slow first turn (finding 13); and the first
note's remaining list (authentication of `RESUME`, MTU and 0-RTT carried across a
resume, `RESUME` landing on another server instance).
