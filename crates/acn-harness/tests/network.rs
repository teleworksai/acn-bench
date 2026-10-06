//! A session over a network (SPEC 020 EMU-32 to EMU-35), against the same
//! session with none: one marker, so one set of prompts.

#![allow(clippy::unwrap_used, clippy::expect_used, clippy::panic)] // CON-19: tests are exempt

mod common;

use std::path::PathBuf;

use acn_harness::wire::Exchange;
use common::{Spec, session};

fn repo(rel: &str) -> PathBuf {
    PathBuf::from(concat!(env!("CARGO_MANIFEST_DIR"), "/../..")).join(rel)
}

fn fanout() -> String {
    std::fs::read_to_string(repo("workloads/p4-fanout.toml")).unwrap()
}

/// What an exchange said: start, end, status, events, body, bytes down and
/// failure, without its link records.
type Seen = (i64, i64, u16, Vec<(i64, String)>, Vec<u8>, u64, String);

fn seen(ex: &[Exchange]) -> Vec<Seen> {
    ex.iter()
        .map(|e| {
            (
                e.start_ns,
                e.end_ns,
                e.status,
                e.events.clone(),
                e.body.clone(),
                e.bytes_down,
                format!("{:?}", e.failure),
            )
        })
        .collect()
}

/// Cites: EMU-33, EMU-34, HAR-41
#[test]
fn a_zero_delay_path_changes_no_exchange() {
    // Forked children call at one instant: over a network of no delay, the
    // instant's phases keep them in one batch, and every exchange (times,
    // events, bodies, failures) is what it is with no network.
    let clean = acn_emu::scenario::load(&repo("scenarios/synthetic/clean.toml")).unwrap();
    let vary = [("fanout_prompting", "fork_from_prefix")];
    let spec = |scenario| Spec {
        workload: fanout(),
        vary: &vary,
        scenario,
        ..Spec::default()
    };
    let none = session(spec(None));
    let net = session(spec(Some(clean)));
    assert_eq!(none.bodies, net.bodies);
    assert_eq!(seen(&none.exchanges), seen(&net.exchanges));
    assert!(none.exchanges.iter().all(|e| e.links.is_empty()));
    assert!(net.exchanges.iter().all(|e| !e.links.is_empty()));
    assert!(none.exchanges.len() > 3);
}

/// Cites: EMU-34, EMU-32
#[test]
fn a_delayed_path_delays_every_timestamp_by_its_delays() {
    // 5 ms up and 5 ms down, no jitter: the first token arrives 10 ms later
    // than with no network, and every event 10 ms after it would arrive with none.
    let text = "schema_version = 1\nname = \"d\"\n\n[[link]]\nname = \"p\"\ndirection = \"up\"\n\n[link.delay]\ndelay_us = 5000\njitter_us = 0\n\n[[link]]\nname = \"p\"\ndirection = \"down\"\n\n[link.delay]\ndelay_us = 5000\njitter_us = 0\n";
    let dir = tempfile::tempdir().unwrap();
    let path = dir.path().join("d.toml");
    std::fs::write(&path, text).unwrap();
    let sc = acn_emu::scenario::load(&path).unwrap();
    let none = session(Spec::default());
    let net = session(Spec {
        scenario: Some(sc),
        ..Spec::default()
    });
    let (a, b) = (&none.exchanges[0], &net.exchanges[0]);
    assert_eq!(a.start_ns, b.start_ns);
    assert!(!a.events.is_empty());
    // The mock answers the same at an arrival 5 ms later, save the response
    // id it derives from the arrival: the times are the point.
    assert_eq!(a.events.len(), b.events.len());
    for ((ta, _), (tb, _)) in a.events.iter().zip(&b.events) {
        assert_eq!(*tb, ta + 10_000_000);
    }
    assert_eq!(b.end_ns, a.end_ns + 10_000_000);
    // Its link records: one request up, one message per event down.
    assert_eq!(b.links.len(), 1 + b.events.len());
}

fn scenario(text: &str) -> (tempfile::TempDir, acn_emu::scenario::Scenario) {
    let dir = tempfile::tempdir().unwrap();
    let path = dir.path().join("s.toml");
    std::fs::write(&path, format!("schema_version = 1\nname = \"s\"\n{text}")).unwrap();
    let sc = acn_emu::scenario::load(&path).unwrap();
    (dir, sc)
}

/// Cites: EMU-33, EMU-35
#[test]
fn a_request_delivered_after_its_timeout_still_reaches_the_mock() {
    // 11 ms up, a 10 ms timeout: every attempt times out before its request
    // arrives, and the mock still sees each one (it builds its cache).
    let (_d, sc) = scenario(
        "\n[[link]]\nname = \"p\"\ndirection = \"up\"\n\n[link.delay]\ndelay_us = 11000\njitter_us = 0\n\n[[link]]\nname = \"p\"\ndirection = \"down\"\n",
    );
    let opts = acn_harness::agent::Opts {
        request_timeout_ms: 10,
        ..acn_harness::agent::Opts::default()
    };
    let ran = session(Spec {
        scenario: Some(sc),
        opts,
        ..Spec::default()
    });
    assert!(ran.exchanges.iter().all(|e| e.failure.is_some()));
    assert_ne!(ran.cache, (0, 0), "the mock never saw a request");
}

/// Cites: EMU-35, EMU-36
#[test]
fn nothing_received_after_an_attempt_ends_belongs_to_it() {
    // A slow downlink (1 000 bytes/s after a 300-byte burst) and a 500 ms
    // timeout: streams time out part-way, keeping only what arrived by then.
    let (_d, sc) = scenario(
        "\n[[link]]\nname = \"p\"\ndirection = \"up\"\n\n[[link]]\nname = \"p\"\ndirection = \"down\"\n\n[link.rate]\nrate_kbps = 8\nburst_bytes = 300\nqueue_bytes = 1000000\n",
    );
    let opts = acn_harness::agent::Opts {
        request_timeout_ms: 500,
        ..acn_harness::agent::Opts::default()
    };
    let ran = session(Spec {
        scenario: Some(sc),
        opts,
        ..Spec::default()
    });
    let cut_short = ran
        .exchanges
        .iter()
        .filter(|e| e.failure.is_some() && !e.events.is_empty())
        .count();
    assert!(cut_short > 0, "no attempt timed out part-way");
    for e in &ran.exchanges {
        assert!(e.events.iter().all(|(t, _)| *t <= e.end_ns));
        let sum: u64 = e.events.iter().map(|(_, d)| d.len() as u64 + 8).sum();
        assert_eq!(e.bytes_down, sum);
        for l in e.links.iter().skip(1) {
            match l.received_ns {
                Some(t) => assert!(t <= e.end_ns),
                None => assert!(l.fate.send_ns <= e.end_ns),
            }
        }
    }
}

/// Cites: EMU-33, HAR-41
#[test]
fn waits_and_ending_attempts_resume_together() {
    // Lineage A makes a call; lineage B sleeps until the instant A's call
    // ends. Without a network both resume in one group, A first (the order
    // of the futures); over a zero-delay network they must too.
    use acn_harness::env::{Env as _, SimEnv, join_all};
    use std::cell::RefCell;
    let body = serde_json::to_vec(&serde_json::json!({
        "model": "auto",
        "messages": [{ "role": "user", "content": "hi" }],
        "max_tokens": 8
    }))
    .unwrap();
    let order = |env: &SimEnv, end: Option<i64>| -> (Vec<&'static str>, i64) {
        let log = RefCell::new(Vec::new());
        let a = async {
            let ex = env
                .exchange("/v1/chat/completions", body.clone(), false, 10_000_000_000)
                .await;
            log.borrow_mut().push("A");
            ex.end_ns
        };
        let b = async {
            if let Some(t) = end {
                env.sleep_until(t).await;
                log.borrow_mut().push("B");
            }
            0
        };
        let ends = env
            .drive(async {
                let a: std::pin::Pin<Box<dyn std::future::Future<Output = i64>>> = Box::pin(a);
                let b: std::pin::Pin<Box<dyn std::future::Future<Output = i64>>> = Box::pin(b);
                join_all(vec![a, b]).await
            })
            .unwrap();
        (log.into_inner(), ends[0])
    };
    let mock = || acn_mockllm::Mock::with_profiles(common::three(), 1).unwrap();
    let (_, end) = order(&SimEnv::new(mock(), "m".into()), None);
    let (plain, _) = order(&SimEnv::new(mock(), "m".into()), Some(end));
    let clean = acn_emu::scenario::load(&repo("scenarios/synthetic/clean.toml")).unwrap();
    let net = SimEnv::with_scenario(mock(), "m".into(), &clean, 1).unwrap();
    let (over, _) = order(&net, Some(end));
    assert_eq!(plain, vec!["A", "B"]);
    assert_eq!(over, plain);
}
