//! TRC-27: in `sim` mode trace and span ids come from the seeded sub-stream
//! `trace.ids` of the replicate, or of the run for the scenario root span.

#![allow(clippy::unwrap_used, clippy::expect_used, clippy::panic)] // CON-19: tests are exempt

use acn_trace::fixture::{self, FixtureRun};
use acn_trace::identity::Digest;
use acn_trace::ids::SeededIdGenerator;
use opentelemetry_sdk::trace::IdGenerator as _;

fn hex(bytes: &[u8]) -> String {
    bytes.iter().map(|b| format!("{b:02x}")).collect()
}

/// Cites: TRC-27, CON-30
#[test]
fn replicate_ids_match_the_known_keystream_of_their_substream() {
    // The ChaCha20 keystream of sub-stream `trace.ids` under replicate_seed(42, 3),
    // computed outside this crate: a trace id is the next 16 bytes, a span id the
    // next 8, in the order they are asked for.
    let g = SeededIdGenerator::for_replicate(42, 3).unwrap();
    assert_eq!(
        hex(&g.new_trace_id().to_bytes()),
        "315df77e2cd5cd1ab2799cc41c7dbf16"
    );
    assert_eq!(hex(&g.new_span_id().to_bytes()), "0739eae91597768f");
}

/// Cites: TRC-27, CON-30
#[test]
fn run_level_ids_draw_from_the_run_seed() {
    let g = SeededIdGenerator::for_run(42).unwrap();
    assert_eq!(
        hex(&g.new_trace_id().to_bytes()),
        "25f49f497385ad6f5c0f98ddd12e0b0e"
    );
}

/// Cites: TRC-27
#[test]
fn replicates_draw_from_separate_streams_and_ids_are_never_invalid() {
    let a = SeededIdGenerator::for_replicate(42, 0).unwrap();
    let b = SeededIdGenerator::for_replicate(42, 1).unwrap();
    assert_ne!(a.new_trace_id(), b.new_trace_id());
    for _ in 0..1000 {
        assert_ne!(a.new_span_id().to_bytes(), [0; 8]);
        assert_ne!(a.new_trace_id().to_bytes(), [0; 16]);
    }
}

fn run(seed: u64, replicate: u32) -> FixtureRun {
    FixtureRun {
        run_id: "r".into(),
        seed,
        replicate,
        engine_hash: Digest::of(b"engine"),
        build_hash: Digest::of(b"build"),
    }
}

/// Cites: TRC-27, TRC-1
#[test]
fn a_producer_on_the_sdk_gets_the_same_ids_on_every_run() {
    let a = fixture::session(&run(7, 0)).unwrap();
    let b = fixture::session(&run(7, 0)).unwrap();
    assert_eq!(a, b, "same seed and replicate, same trace, ids included");
    let c = fixture::session(&run(7, 1)).unwrap();
    assert_ne!(a.spans[0].trace_id, c.spans[0].trace_id);
    // The structure is the SDK's: every span but the root has a parent in the trace.
    let roots: Vec<_> = a
        .spans
        .iter()
        .filter(|s| s.parent_span_id.is_none())
        .collect();
    assert_eq!(roots.len(), 1);
    assert_eq!(roots[0].name, "acn.session");
    for s in &a.spans {
        if let Some(p) = s.parent_span_id {
            assert!(
                a.spans.iter().any(|x| x.span_id == p),
                "{} has no parent",
                s.name
            );
        }
    }
}
