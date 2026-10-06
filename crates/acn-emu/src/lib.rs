//! `acn-emu` — link models, scenario loader, sim engine and live proxy (SPEC 020).
//!
//! T03 added the injected [`clock`] of CON-5(b), the only place a run reads time
//! (ADR-16). T08 added measured impairment [`trace`]s (SPEC 020 §6). T10 adds
//! the [`link`] models (§2) and the synthetic [`scenario`] loader (§3). The sim
//! engine and the live proxy come with T11 and T12.
#![forbid(unsafe_code)]

pub mod clock;
pub mod link;
pub mod scenario;
pub mod trace;
