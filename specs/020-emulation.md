# SPEC 020 — Link emulation: models, engine, proxy and measured traces

**Status:** Draft v0.0 (October 2026). Partial: only §6, measured traces, is written, for T08. The rest of the spec (link models, the sim engine, the live proxy, scenarios) is T10's to T12's and takes IDs below EMU-60. **Inherits:** SPEC 000, 010. **Prefix:** EMU. **Crate:** `acn-emu` (Class B); `scenarios/measured/` is in the frozen set (CON-7).
**Purpose (of §6):**
- define what a measured impairment trace is, so that CON-21's provenance rule and its ban on payload and identifiers can be checked by a machine;
- define how the first trace, a published 5G drive test (ADR-29), enters the repository.

## 6. Measured traces

**EMU-60** A measured trace MUST be a directory `scenarios/measured/<slug>/`, with `<slug>` lowercase ASCII letters, digits and `-`, holding exactly two files: `trace.toml` and `provenance.toml`. Both are parsed with unknown keys refused at every level. Both are hashed inputs, read as working-tree bytes (CON-27(a)).

**EMU-61** `provenance.toml` MUST state:
- the source: a title, a URL, and a persistent identifier (DOI or record number) when there is one;
- the licence as an SPDX identifier, and the attribution the licence asks for;
- the collection date, the device, the network technology, and whether the network is a testbed or commercial;
- the location class (for example `suburban road`), never coordinates;
- the mobility (`static`, `walk` or `drive`);
- the method: how the source measured;
- the conversion:
  - the tool that turned the source into `trace.toml`, as a path in this repository and the BLAKE3 of that source file (a hash, not a commit, because the conversion lands in the same change as the trace);
  - the name and BLAKE3 of every source file it read;
- what the conversion dropped, and why.

A licence that does not allow redistribution MUST be refused.

**EMU-62** `trace.toml` MUST hold `schema_version = 1`, a `sample_interval_s` (the nominal spacing of samples), and an ordered array of `[[sample]]` tables. A sample MUST carry only these fields:
- `t_s`, seconds since the first sample, which is 0. It is strictly increasing.
- `loss`, a fraction in `[0, 1]`.
- `rtt_ms`, a table `{ avg, min, max, stdev }` with `0 ≤ min ≤ avg ≤ max` and `stdev ≥ 0`. It is present exactly when `loss < 1`: a sample in which no probe returned has no round-trip time, and still belongs to the trace (an outage is the impairment a trace exists to record).
- `dl_kbps` and `ul_kbps`, the throughput measured downlink and uplink, each ≥ 0.

Every number MUST be finite. A trace MUST hold at least two samples.

**EMU-63** A measured trace MUST NOT contain payload data or identifiers (CON-21). The fixed field set of EMU-62 enforces this: no coordinates, no cell or device identifier, no address and no wall-clock time. Times are relative to the first sample.

**EMU-64** `acn_emu::trace::load` MUST read a trace directory and refuse it with a named reason when either file breaks EMU-60 to EMU-63. It MUST also return the trace's hash: the BLAKE3 of `trace.toml`, which scenarios will name it by (CON-27(a)).

**EMU-65** The first measured trace (ADR-29) is `scenarios/measured/5g-iana-2023-01-29/`, converted from the PING file of Zenodo record 12664724 (CC BY 4.0). Its conversion tool is `cargo xtask import-5g-iana`. Run on the published source files, the tool MUST reproduce `trace.toml` byte for byte, and refuse a source whose BLAKE3 differs from the one recorded in `provenance.toml`.

## 9. Acceptance tests (§6)

- `crates/acn-emu/tests/measured.rs` — EMU-60 to EMU-64: every trace under `scenarios/measured/` loads; each malformed variant (an unknown key, a decreasing `t_s`, `min > avg`, a loss above 1, `rtt_ms` with a loss of 1 or without one below 1, a non-finite number, a coordinate field, a missing provenance field, a non-redistributable licence) is refused by name.
- `crates/xtask/tests/import_5g_iana.rs` — EMU-65: the importer, run on a source with the published structure, writes the documented samples. Its refusal of a source whose hash differs from the recorded one is tested.

## 10. Open questions (ADR candidates)

1. How a scenario uses a measured trace: replayed sample by sample, or fitted to a link model of T10. This is T10's question.
2. Whether `scenarios/measured/` should also hold our own captures (T08's original plan, a phone-tethered walk) in the same format, with `mobility = "walk"`. That is expected; the format does not depend on the source.
