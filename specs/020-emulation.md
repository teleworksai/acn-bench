# SPEC 020 — Link emulation: models, engine, proxy and measured traces

**Status:** Draft v0.0 (October 2026). Partial: only §6, measured traces, is written, for T08. The rest of the spec (link models, the sim engine, the live proxy, scenarios) is T10's to T12's and takes IDs below EMU-60. **Inherits:** SPEC 000, 010. **Prefix:** EMU. **Crate:** `acn-emu` (Class B); `scenarios/measured/` is in the frozen set (CON-7).
**Purpose (of §6):**
- define what a measured impairment trace is, so that CON-21's provenance rule and its ban on payload and identifiers can be checked by a machine;
- define how the first trace, a published 5G drive test (ADR-29), enters the repository.

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

## 9. Acceptance tests (§6)

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

1. How a scenario uses a measured trace: replayed sample by sample, or fitted to a link model of T10. This is T10's question.
2. Whether `scenarios/measured/` should also hold our own captures (T08's original plan, a phone-tethered walk) in the same format, with `mobility = "walk"`. That is expected; the format does not depend on the source.
