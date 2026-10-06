# Specs index

| # | File | Prefix | Status |
|---|---|---|---|
| 000 | 000-constitution.md | CON | draft v0.2 |
| 010 | 010-trace-schema.md | TRC | draft v0.2 (ACN profile of OpenTelemetry; ADR-2) |
| 020 | 020-emulation.md | EMU | draft v0.4 (§2 link models and trace-driven links, §3 scenario files, §4 the sim engine, §5 the live proxy, for T10 to T12; §6 measured traces, for T08) |
| 030 | 030-mock-inference.md | MLM | draft v0.1 (mock inference: wire format, prompt tokens, three cache models, timing, replies; implemented by T03) |
| 040 | 040-harness.md | HAR | draft v0.3 (harness: agent loop, six cache-discipline knobs, two wire dialects, recording, isolation, workloads; implemented by T04) |
| 050 | 050-workload-generator.md | GEN | to write (T13) |
| 060 | 060-replayer.md | RPL | to write (T50) |
| 070 | 070-control-plane.md | CTL | to write (T14) |
| 080 | 080-hypotheses.md | HYP | draft v0.1 (format, typed predicate grammar, slices and verdicts; implemented by T05) |
| 085 | 085-feedback-loop.md | LOOP | draft v0.3 (layered, verifiable loop; L1 runner, report and evidence chain; v0.3 thins the verdict trajectory; implemented by T05b, T06b3) |
| 090 | 090-attribution.md | ATR | to write (T15) |
| 095 | 095-gates.md | GATE | draft v0.1 (§1 rules for every gate and §2 gate M0, written for T07; M1–M4 sections to write with T17 and later gates) |
| 100 | 100-p4-harness-discipline.md | P4 | draft v0.1 (POC 4 protocol: workloads, providers and mock profiles, the L1 run of record, the mock acceptance suite, live-run prerequisites; implemented by T06) |
| 110 | 110-p1a-sensitivity-atlas.md | P1A | to write (T20) |
| 111 | 111-p1b-tail-anatomy.md | P1B | to write (T21) |
| 120 | 120-p11-wire-redundancy.md | P11 | to write (T22) |
| 121 | 121-p12-stream-survivability.md | P12 | to write (T42) |
| 122 | 122-p13-fanout-burst.md | P13 | to write (T23) |
| 123 | 123-p14-protocol-chattiness.md | P14 | to write (T43) |
| 130 | 130-p7-affinity-cost-curve.md | P7 | to write (T31) |
| 140+ | one per remaining POC (2a, 3a, 5, 6, 8, 9, 10, 15, 16) | P… | later |

Specs are read-only for agents (CON-14). Every MUST carries an ID `<PREFIX>-<n>`; IDs are never reused.
