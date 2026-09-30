# ADR-1 — Userspace impairment proxy as the primary `live` backend

**Status:** accepted (draft v0.1). **IDs affected:** CON-1, EMU (SPEC 020, to write).

## Context
The primary development platform is macOS arm64 (CON-1). Linux netem/tc requires root and network namespaces and does not exist on macOS. The experiments need a network boundary with deterministic, seedable impairments (CON-5).

## Decision
`acn-emu` implements link models once (`LinkModel` trait) and applies them in three backends: `sim` (discrete-event, virtual clock, no sockets — the ACN simulator), `live` (a tokio userspace TCP proxy between real sockets, schedule precomputed from the seed), and `netem` (Linux, feature-gated, added at M4 to validate `live` against kernel-level impairment on the measured traces).

## Consequences
`sim` and `live` run on every developer machine and in CI. Kernel-level effects (TCP stack interaction with loss, HTTP/2 and QUIC behaviour under reordering) are approximated in `live` and validated in `netem` at M4; results published before M4 are labelled accordingly in the bundle manifest (`backend = "live"`).
