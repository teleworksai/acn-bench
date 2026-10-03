//! HAR-40..43: the replicate's sub-streams, simultaneous sub-agent calls in one
//! batch, isolation markers, and the seeded execution order.

#![allow(clippy::unwrap_used, clippy::expect_used, clippy::panic)] // CON-19: tests are exempt

mod common;

use acn_harness::agent::Streams;
use acn_harness::run::isolation_marker;
use acn_trace::identity::{Digest, Mode};
use common::{MARKER, Spec, children, float, int, profile, profiles, run_fixture, session, spans};
use rand_core::Rng as _;

/// Cites: HAR-40, CON-30
#[test]
fn every_draw_comes_from_a_named_sub_stream_of_the_replicate() {
    let mut s = Streams::new(42).unwrap();
    for (name, rng) in [
        ("harness.tools", &mut s.tools),
        ("harness.knobs", &mut s.knobs),
        ("harness.workload", &mut s.workload),
    ] {
        let mut expected = acn_trace::identity::substream_rng(42, name).unwrap();
        assert_eq!(rng.next_u64(), expected.next_u64(), "{name}");
    }
    let bytes = |seed| {
        spans(
            &session(Spec {
                seed,
                ..Spec::default()
            })
            .trace,
            "execute_tool",
        )
        .iter()
        .map(|t| int(t, "acn.tool.result_bytes").unwrap())
        .collect::<Vec<_>>()
    };
    assert_eq!(bytes(1), bytes(1));
    assert_ne!(bytes(1), bytes(2), "another replicate seed, other results");
}

/// Cites: HAR-41, MLM-7
#[test]
fn simultaneous_sub_agent_calls_reach_the_mock_as_one_batch() {
    // One slot: of two calls due at once, MLM-7's order decides which queues.
    let run = || {
        session(Spec {
            task: 1,
            profiles: profiles(&[profile("auto", "automatic_prefix", &["slots = 1"])]),
            ..Spec::default()
        })
    };
    let (a, b) = (run(), run());
    assert_eq!(a.bodies, b.bodies);
    let first_calls = |r: &common::Ran| {
        spans(&r.trace, "invoke_agent")
            .iter()
            .map(|agent| {
                let c = children(&r.trace, agent)
                    .into_iter()
                    .find(|s| s.name == "chat")
                    .unwrap()
                    .clone();
                (c.start_ns, float(&c, "acn.call.ttft_ms").unwrap())
            })
            .collect::<Vec<_>>()
    };
    let fa = first_calls(&a);
    assert_eq!(fa[0].0, fa[1].0, "submitted together");
    assert_ne!(fa[0].1, fa[1].1, "one of them waited for the slot");
    assert_eq!(fa, first_calls(&b), "and always the same one");
}

/// Cites: HAR-42, CON-27
#[test]
fn every_run_arm_and_replicate_has_its_own_marker_in_every_cache_order() {
    let run_id = Digest::of(b"run");
    // Known answer: blake3("acn-bench/harness_isolation/v1\0" ‖ run_id ‖ len32("treatment") ‖ "treatment" ‖ u32 3).
    let mut pre = b"acn-bench/harness_isolation/v1\0".to_vec();
    pre.extend_from_slice(blake3::hash(b"run").as_bytes()); // = run_id's raw bytes
    pre.extend_from_slice(&9u32.to_le_bytes());
    pre.extend_from_slice(b"treatment");
    pre.extend_from_slice(&3u32.to_le_bytes());
    let expected = blake3::hash(&pre).to_hex()[..16].to_owned();
    assert_eq!(isolation_marker(&run_id, "treatment", 3).unwrap(), expected);
    assert_ne!(
        isolation_marker(&run_id, "control", 3).unwrap(),
        expected,
        "arms differ"
    );
    assert_ne!(isolation_marker(&run_id, "treatment", 4).unwrap(), expected);
    // In the system prompt's first line and at the start of every tool description.
    let r = session(Spec::default());
    for (_, b) in &r.bodies {
        let sys = b["messages"][0]["content"][0]["text"].as_str().unwrap();
        assert!(sys.starts_with(&format!("Session: {MARKER}\n")));
        for t in b["tools"].as_array().unwrap() {
            let d = t["function"]["description"].as_str().unwrap();
            assert!(d.starts_with(MARKER), "{d}");
        }
    }
}

/// Cites: HAR-43, CON-5, MLM-60
#[test]
fn replicates_run_in_the_seeded_order_and_live_records_it() {
    let workload = common::smoke()
        .replace(
            "think_time_ns = { min = 1_000_000_000, max = 3_000_000_000 }",
            "think_time_ns = { min = 0, max = 0 }",
        )
        .replace(
            "think_time_ns = { min = 500_000_000, max = 1_500_000_000 }",
            "think_time_ns = { min = 0, max = 0 }",
        );
    let mut f = run_fixture(&workload, "auto");
    f.cfg.replicates = 4;
    f.cfg.mode = Mode::Live;
    f.cfg.opts.endpoint = common::mock_server(common::three());
    let w = acn_harness::run::run(&f.cfg).unwrap();
    let manifest: serde_json::Value =
        serde_json::from_slice(&std::fs::read(w.dir.join("manifest.json")).unwrap()).unwrap();
    let mut rng = acn_trace::identity::substream_rng(7, "run.order").unwrap();
    let order: Vec<String> = acn_harness::agent::permutation(&mut rng, 4)
        .iter()
        .map(|i| format!("treatment/{i}"))
        .collect();
    assert_eq!(manifest["execution_order"], serde_json::json!(order));
    assert!(manifest["started_at"].as_str().unwrap().ends_with('Z'));
    assert_eq!(manifest["endpoint_host"], "127.0.0.1");
    // The sessions ran in that order on the wall clock.
    let trace = common::read(&w.dir);
    let mut sessions: Vec<(i64, i64)> = spans(&trace, "acn.session")
        .iter()
        .map(|s| (s.start_ns, int(s, "acn.replicate").unwrap()))
        .collect();
    sessions.sort_unstable();
    let ran: Vec<String> = sessions
        .iter()
        .map(|(_, i)| format!("treatment/{i}"))
        .collect::<Vec<_>>()
        .chunks(2)
        .map(|c| c[0].clone())
        .collect();
    assert_eq!(ran, order);
    // In sim the order is drawn the same way but not recorded (TRC-22).
    let sim = run_fixture(&workload, "auto");
    let w = acn_harness::run::run(&sim.cfg).unwrap();
    let m: serde_json::Value =
        serde_json::from_slice(&std::fs::read(w.dir.join("manifest.json")).unwrap()).unwrap();
    assert!(m.get("execution_order").is_none());
}
