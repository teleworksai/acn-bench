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
