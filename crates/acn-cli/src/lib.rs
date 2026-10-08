//! The parts of `acn` other crates' tests run exactly as the binary does: the
//! harness executor `acn loop run` and `acn evidence verify` hand the loop
//! runner (LOOP-15).
#![forbid(unsafe_code)]

pub mod attrib;
pub mod loop_exec;
pub mod regen;
