# ADR-36 — T12: an HTTP-aware live proxy, one per replicate

**Status:** accepted (T12.1; `spec-change`, SPEC 020 Draft v0.4 §5). **IDs affected:** EMU-40 to EMU-49; EMU-9, EMU-32 to EMU-37; CON-5(d), CON-8, CON-25, CON-29; TRC-15, TRC-26. **Settles:** SPEC 020 §10 question 3.

## Context
T12 applies the link models of §2 to real sockets: the `live` half of every sim↔live twin (CON-25). A `live` harness talks HTTP/1.1, over reqwest, to an endpoint: the mock server or a provider. The sim network of §4 carries messages framed by HTTP meaning: a request, a body, an SSE event.

On a TCP byte stream the questions are different:
- what counts as a message;
- what a drop can mean without corrupting HTTP;
- how delivery times are kept on a wall clock.

## Decision
- **The proxy is HTTP-aware and frames messages as the sim does (EMU-41).** A request, a body, or an SSE event of the response is one message, of the same bytes EMU-32 counts.
  - Framing by reads and writes would make the number of messages depend on kernel buffering and the endpoint's chunk sizes, which no sim run could reproduce. The twin would then compare different things.
  - The mock writes one HTTP chunk per SSE event; real providers need not. So the proxy re-frames events from the byte stream, by their blank-line ends.
- **Drops are turned into what the client sees in sim (EMU-44).** Bytes cannot be cut out of a TCP stream.
  - A lost request or body leaves the connection silent, so the attempt times out, as in EMU-35.
  - A lost event aborts the connection when the next delivered event would arrive, so the client sees a broken response and retries, as a cut does in sim. That holds for the first event too, whose loss leaves the head unwritten. A first draft held the head back and left the connection silent, so the same draws gave a timeout live and a cut in sim; the review caught it.
  - The proxy never delimits a body by closing the connection, so an abort always reaches the client as an error rather than a clean end.
  - Holds, delays, rate waits and reorder gaps all become later write times. Within one connection, writes are in order, so EMU-34's order rule holds by itself. Across connections, a reordered request is really overtaken.
- **Timing (EMU-42, EMU-43).** The proxy reads the run's `Clock`, a `WallClock` shared in-process, so its times need no offset (TRC-26), and no new reader of the process clock appears outside `acn_emu::clock`.
  - A message's send time is taken when the proxy has read it in full, in the same step as offering it to its link, so a link's send times never decrease.
  - A write waits for the delivery time on the clock, and is never early. Timer slack makes it later by up to a few milliseconds, and the recorded receive time is the actual write time. A spin-wait for sub-millisecond accuracy is not done; if it ever is, it is a run option (CON-29).
- **The impairment schedule is precomputed (CON-5(d), EMU-40).** EMU-9 makes every draw depend on the message index alone. Building each link from the replicate seed before the proxy accepts a connection therefore fixes the whole schedule before traffic starts. What remains time-dependent (rates, holds, the order of concurrent messages) is the network's response to the traffic itself, not a random choice.
- **Upstream (EMU-45).**
  - One upstream connection per downstream connection, opened at its first forward. Its setup, TLS included, counts as server time after the delivery, not as uplink time.
  - A refused or reset upstream closes the downstream connection, so the client sees a transport error.
  - Forwarding runs apart from the downstream connection's task. A client that gives up does not cancel a request already delivered (EMU-33).
  - TLS to a provider is compiled only with `real-api` (HAR-20), through `hyper-rustls`, which the lock file already holds.
- **Transparency (EMU-46).** `Host` is set to the endpoint's, its base path is prefixed, hop-by-hop headers are dropped, the attempt header is stripped, and no header or body is ever logged (HAR-22).
- **Records on the run's clock (EMU-47).** A live run has one clock from its start, and each replicate's links count from that replicate's origin. So the records' times, and the spans and scenario events made from them, are the origin plus the link time. In sim the origin is 0, so nothing there changes.
- **One proxy per replicate, in-process (EMU-40, EMU-49).** Each live replicate gets its own links, its own origin at its start, and a listener on `127.0.0.1:0`, shut down when the replicate ends, which also releases any connection held silent.
  - The run's `opt.endpoint` and endpoint host keep naming the real endpoint, so the proxy's ephemeral port never enters `run_id`.
  - A standalone `acn proxy` command is not built now. A long-running server does not fit CON-8's one-JSON-object contract, and the workload generator (T13) and control plane (T14) can host the library when they need it.
- **The harness emits the records, as in sim (EMU-47).** The client tags each attempt with `x-acn-attempt`, which the proxy strips before forwarding; this is the "flow map" TRC-15 allows for opaque traffic. The proxy keeps the per-message records by attempt, and the harness collects an attempt's records when it ends. The span code of T11.3 (`Exchange.links`, the link spans, the scenario events) is then shared by sim and live.
- **What the twin can claim (EMU-48).** Per message index, the same raw draws. They are the same decisions only on links whose parameters do not depend on time, and only when both runs offer messages in the same order. Send-time-dependent stages (rates, holds, outage windows, trace segments) and the order of concurrent messages, including a timed-out attempt's late response, are compared as distributions by T11b.

## Consequences
- **Dependencies.** `acn-emu` gains HTTP dependencies (`hyper`, `hyper-util`, `http-body-util`, and `hyper-rustls` under `real-api`), already in the lock file through other crates. `cargo deny` must clear them.
- **What T11.3's span code needs.** It must add the replicate's origin to a live record's times.
- **Twin divergence floor.** About a millisecond of timer slack, plus the loopback's own latency, enters every twin divergence. Scenarios whose effects are below a few milliseconds cannot be told apart from it.
- **Live runs over providers' SSE.** These need the proxy to frame events exactly as written (`event:` lines, `\r\n` endings); T12.2 tests such events.
