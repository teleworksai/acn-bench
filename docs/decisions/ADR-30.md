# ADR-30 — T08: the first measured trace, from the 5G-IANA drive test

**Status:** accepted (T08; `spec-change`, SPEC 020 Draft v0.0 §6, and `env-change`, `scenarios/measured/`). **IDs affected:** EMU-60 to EMU-65; CON-7, CON-21, CON-27(a). **Follows:** ADR-29.

## Context
ADR-29 made the published 5G-IANA drive test (Zenodo record 12664724, CC BY 4.0) the first measured trace, until our own capture exists. CON-21 asks that a measured trace carry its provenance and no payload or identifiers. Nothing yet said what a trace file holds. SPEC 020 was unwritten, and T10 owns its link models, engine and proxy. T08 needed:
- a format;
- a loader that checks the format;
- a way to show the committed trace is the source, converted.

## Decision
- **SPEC 020 is written partially.** Only §6, with IDs EMU-60 to EMU-65, is written. IDs below EMU-60 are left to T10–T12. How a scenario uses a trace (replay or fit) is T10's open question (§10 Q1).
- **Fixed field set (EMU-62, EMU-63).** A sample holds only these fields: `t_s`, `loss`, `dl_kbps`, `ul_kbps` and `rtt_ms {avg, min, max, stdev}`.
  - Unknown keys are refused at every level, so a coordinate, a cell id or a wall-clock time cannot be added without a spec change.
  - Times are relative to the first sample.
- **Outages stay in the trace.** 19 of the 198 tests with a ping lost every probe and report no round-trip time; 14 of them are the last 14 tests, where downlink throughput falls to 0 and uplink to nearly 0. Dropping them would remove the impairment a trace exists to record. Instead, `rtt_ms` is present exactly when `loss < 1`, and the loader checks both directions.
- **What the conversion drops.** It drops the first placemark, which ran throughput tests but no ping test and so has no loss. It also drops the peak throughputs (`TEST DL MAX`, `TEST UL MAX`). `provenance.toml` lists every drop, with its reason.
- **Only the PING file is read.** The record's DL, UL and PING files hold identical placemarks and fields; they differ only in their KML colour styling. So the conversion reads only the PING file, and `provenance.toml` records that file's name and BLAKE3.
- **Source bytes stay out of the repo.** The source carries GPS coordinates and cell ids (EMU-63).
  - What the repo has:
    - the converted trace;
    - the source's hash;
    - the importer;
    - a four-placemark fixture of the published structure, with coordinates and cell ids zeroed, but times kept (the importer needs them). An XML comment in it gives the CC BY attribution and says what was changed.
  - The `#[ignore]` test `the_committed_trace_is_the_importers_output_on_the_source` runs `--check` on the downloaded source, named by `ACN_5G_IANA_PING`. It passed when T08 landed.
- **The tool is named by its file hash (EMU-61).** `provenance.toml` records the BLAKE3 of `crates/xtask/src/import_5g_iana.rs`. The importer refuses to run if that hash differs, and the integration test `the_committed_provenance_names_this_tool_by_its_hash` checks it on every `cargo test`.
  - Editing the importer therefore forces a re-conversion and a new hash in the frozen set, an `env-change`. That is intended, because a changed converter could change the trace.
- **`sample_interval_s` is the lower median of the gaps** between kept tests: 26 s here. The gaps run from 13 s to 98 s, so it is nominal, as EMU-62 says.
- **Numbers are written in their shortest round-trip form** (Rust's `{:?}` for `f64`, with `.0` on integers). `--check` is therefore a byte comparison.
- **The redistributable licences** are an explicit SPDX allow-list in `acn_emu::trace::REDISTRIBUTABLE`. Any other licence is refused; adding one is a reviewed change.
- **`scenarios/measured/.gitkeep` is removed**, now that the directory holds a trace.

- **The importer checks its own output.** A converted trace is run through the loader's EMU-62 checks (`acn_emu::trace::parse_trace`) before it is written or compared. The importer also reads `provenance.toml` with the loader's schema, refuses a placemark off the published shape, and writes through a temporary file and a rename. EMU-65 states these refusals.
- **Provenance is not in the trace's hash.** Scenarios name a trace by the BLAKE3 of `trace.toml`, which is the run input. `provenance.toml` is not read by a run; the frozen set's `env-hash` commits to it (EMU-60). Chaining it into the trace's hash would make a licence wording fix change every scenario that names the trace.
- **Free text is reviewed, not checked.** The comments in both files and the descriptive provenance fields could hold an identifier that no machine check would catch. EMU-63 makes keeping them clean an obligation of the Class C review.
- **Re-linking to the public source is possible, and accepted.** Each sample keeps the source's exact values, so anyone holding the published file can map a sample back to its GPS point and cell. The source is public under CC BY, published by the network's operator, and identifies no person. EMU-63 asks that this repository hold no identifiers, which it meets.

## Consequences
- `scenarios/measured/5g-iana-2023-01-29/` is in the frozen set, so `env-hash.json` changes, and so does any future edit to the importer.
- The trace is a drive on a testbed, not the walk on a commercial network that T08 planned. §10 Q2 expects our own captures to use the same format.
- Scenarios cannot use the trace yet: T10 decides how a link model consumes it.
- The example requirement IDs in `CLAUDE.md` and `PLAN.md` used EMU numbers below 60. They were forward references while SPEC 020 was unwritten, and would now dangle, so they name `EMU-60` and `EMU-61` instead.
