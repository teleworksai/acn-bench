//! `acn-emu` — link models, scenario loader, sim engine and live proxy (SPEC 020).
//!
//! The link models, scenarios, engine and proxy arrive with T10–T12. T03 adds the
//! one piece the mock inference server needs first: the injected [`clock`] of
//! CON-5(b), the only place a run reads time (ADR-16).
#![forbid(unsafe_code)]

pub mod clock;
