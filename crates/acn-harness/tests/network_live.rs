//! Live runs through the proxy (SPEC 020 EMU-47 to EMU-49), against the mock
//! served on the loopback interface: link spans on the run's clock, drops as
//! timeouts and cut streams, and the same draws per message as a sim run.

#![allow(clippy::unwrap_used, clippy::expect_used, clippy::panic)] // CON-19: tests are exempt

mod common;

use acn_harness::run::run_with_scenario;
use acn_trace::identity::Mode;
use acn_trace::model::{AttrValue, SpanRow, Trace};
use common::{fast_smoke, int, run_fixture, spans, text};

fn scenario(dir: &std::path::Path, name: &str, links: &str) -> std::path::PathBuf {
    let path = dir.join(format!("{name}.toml"));
    std::fs::write(
        &path,
        format!("schema_version = 1\nname = \"{name}\"\n{links}"),
    )
    .unwrap();
    path
}

fn flag(s: &SpanRow, key: &str) -> Option<bool> {
    match s.attrs.get(key)? {
        AttrValue::Bool(b) => Some(*b),
        _ => None,
    }
}

const DELAYED: &str = "\n[[link]]\nname = \"p\"\ndirection = \"up\"\n\n[link.delay]\ndelay_us = 20000\njitter_us = 0\n\n[[link]]\nname = \"p\"\ndirection = \"down\"\n\n[link.delay]\ndelay_us = 20000\njitter_us = 0\n";

/// Cites: EMU-47, EMU-40, EMU-36
#[test]
fn a_live_run_records_its_link_spans_on_the_run_clock() {
    let mut f = run_fixture(&fast_smoke(), "auto");
    f.cfg.mode = Mode::Live;
    f.cfg.replicates = 2;
    f.cfg.opts.endpoint = common::mock_server(common::three());
    let sc = scenario(f.dir.path(), "delayed", DELAYED);
    let w = run_with_scenario(&f.cfg, Some(&sc)).unwrap();
    let t: Trace = common::read(&w.dir);
    let links = spans(&t, "acn.link");
    assert!(!links.is_empty());
    let chats = spans(&t, "chat");
    for l in &links {
        let parent = chats
            .iter()
            .find(|c| Some(c.span_id) == l.parent_span_id)
            .expect("a link's parent is a chat");
        let (enq, deq) = (
            int(l, "acn.link.enqueue_ns").unwrap(),
            int(l, "acn.link.dequeue_ns").unwrap(),
        );
        // On the run's clock, inside the call it carried.
        assert_eq!(l.start_ns, enq);
        assert!(
            enq >= parent.start_ns && deq <= parent.end_ns,
            "{enq}..{deq} outside the call"
        );
        if flag(l, "acn.link.dropped") == Some(false) {
            assert!(deq - enq >= 20_000_000, "less than the link's delay");
        }
    }
    // Every call crossed both links: at least 40 ms.
    for c in &chats {
        assert!(c.end_ns - c.start_ns >= 40_000_000);
    }
    // The manifest names the real endpoint, never the proxy (EMU-49).
    let manifest: serde_json::Value =
        serde_json::from_slice(&std::fs::read(w.dir.join("manifest.json")).unwrap()).unwrap();
    assert_eq!(manifest["endpoint_host"], "127.0.0.1");
    assert_eq!(spans(&t, "acn.scenario").len(), 1);
}

/// Cites: EMU-44, EMU-47
#[test]
fn a_lost_live_request_times_out_and_a_lossy_stream_is_retried() {
    let mut f = run_fixture(&fast_smoke(), "auto");
    f.cfg.mode = Mode::Live;
    f.cfg.opts.endpoint = common::mock_server(common::three());
    f.cfg.opts.request_timeout_ms = 300;
    let down = scenario(
        f.dir.path(),
        "updown",
        "\n[[link]]\nname = \"p\"\ndirection = \"up\"\n\n[[link.outage.window]]\nstart_ms = 0\nend_ms = 60000\nmode = \"drop\"\ncause = \"scheduled\"\n\n[[link]]\nname = \"p\"\ndirection = \"down\"\n",
    );
    let w = run_with_scenario(&f.cfg, Some(&down)).unwrap();
    let t = common::read(&w.dir);
    let first = spans(&t, "chat")
        .into_iter()
        .min_by_key(|c| c.start_ns)
        .unwrap();
    assert_eq!(text(first, "acn.call.error_class"), Some("timeout"));
    let outages: Vec<_> = t
        .events
        .iter()
        .filter(|e| e.name == "acn.scenario.outage")
        .collect();
    assert_eq!(outages.len(), 1);

    let mut f = run_fixture(&fast_smoke(), "auto");
    f.cfg.mode = Mode::Live;
    f.cfg.opts.endpoint = common::mock_server(common::three());
    // A stream whose last events are lost stays silent until the deadline
    // (EMU-44): on the wall clock, so keep it short.
    f.cfg.opts.request_timeout_ms = 2_000;
    let lossy = scenario(
        f.dir.path(),
        "lossy",
        "\n[[link]]\nname = \"p\"\ndirection = \"up\"\n\n[[link]]\nname = \"p\"\ndirection = \"down\"\n\n[link.loss]\nkind = \"iid\"\nloss_ppm = 250000\n",
    );
    let w = run_with_scenario(&f.cfg, Some(&lossy)).unwrap();
    let t = common::read(&w.dir);
    assert!(
        spans(&t, "chat")
            .iter()
            .any(|c| int(c, "acn.call.retries").unwrap_or(0) > 0)
    );
    assert!(
        spans(&t, "acn.link")
            .iter()
            .any(|l| flag(l, "acn.link.dropped") == Some(true))
    );
}

/// Cites: EMU-48, EMU-9
#[test]
fn a_sim_run_and_its_live_twin_take_the_same_draws_per_message() {
    // A link whose parameters do not depend on time: every request and every
    // response message is selected for reordering, or not, by its index alone.
    let links = "\n[[link]]\nname = \"p\"\ndirection = \"up\"\n\n[link.reorder]\nreorder_ppm = 400000\ngap_us = 1000\n\n[[link]]\nname = \"p\"\ndirection = \"down\"\n\n[link.reorder]\nreorder_ppm = 400000\ngap_us = 1000\n";
    // Each direction's flags in the order the network offered the messages.
    // Messages sent at one instant in `sim` (forked children) were offered in
    // an order the spans do not keep, so such a group is compared as a set.
    type Groups = Vec<Vec<bool>>;
    let selected = |mode: Mode| -> (Groups, Groups) {
        let mut f = run_fixture(&fast_smoke(), "auto");
        f.cfg.mode = mode;
        if mode == Mode::Live {
            f.cfg.opts.endpoint = common::mock_server(common::three());
        }
        let sc = scenario(f.dir.path(), "twin", links);
        let w = run_with_scenario(&f.cfg, Some(&sc)).unwrap();
        let t = common::read(&w.dir);
        let of = |dir: &str| -> Groups {
            let mut ls: Vec<(i64, bool)> = spans(&t, "acn.link")
                .iter()
                .filter(|l| text(l, "acn.link.direction") == Some(dir))
                .map(|l| {
                    (
                        int(l, "acn.link.enqueue_ns").unwrap(),
                        flag(l, "acn.link.reordered").unwrap(),
                    )
                })
                .collect();
            ls.sort_unstable();
            let mut groups: Groups = Vec::new();
            let mut last = None;
            for (t, f) in ls {
                if last == Some(t) {
                    if let Some(g) = groups.last_mut() {
                        g.push(f);
                    }
                } else {
                    groups.push(vec![f]);
                }
                last = Some(t);
            }
            groups
        };
        (of("up"), of("down"))
    };
    let (sim_up, sim_down) = selected(Mode::Sim);
    let (live_up, live_down) = selected(Mode::Live);
    // Live sends no two messages at one instant: regroup it as sim grouped.
    let regroup = |sim: &Groups, live: &Groups| -> (Groups, Groups) {
        let flat: Vec<bool> = live.iter().flatten().copied().collect();
        let mut out = Vec::new();
        let mut k = 0;
        for g in sim {
            if k + g.len() > flat.len() {
                break;
            }
            let mut a = g.clone();
            let mut b = flat[k..k + g.len()].to_vec();
            a.sort_unstable();
            b.sort_unstable();
            out.push((a, b));
            k += g.len();
        }
        out.into_iter().unzip()
    };
    for (sim, live) in [(&sim_up, &live_up), (&sim_down, &live_down)] {
        assert!(sim.iter().flatten().any(|x| *x), "no message was selected");
        let (a, b) = regroup(sim, live);
        assert!(a.len() > 3);
        assert_eq!(a, b);
    }
}

/// Cites: EMU-49, EMU-39
#[test]
fn a_live_run_without_a_scenario_goes_straight_to_its_endpoint() {
    let mut f = run_fixture(&fast_smoke(), "auto");
    f.cfg.mode = Mode::Live;
    f.cfg.opts.endpoint = common::mock_server(common::three());
    let w = run_with_scenario(&f.cfg, None).unwrap();
    let t = common::read(&w.dir);
    assert!(spans(&t, "acn.link").is_empty());
    assert!(spans(&t, "acn.scenario").is_empty());
    assert!(
        !t.resources
            .iter()
            .any(|r| r.attrs.get("service.name") == Some(&AttrValue::String("acn-emu".into())))
    );
    for s in spans(&t, "acn.session") {
        assert_eq!(text(s, "acn.scenario.hash"), Some("0".repeat(64).as_str()));
    }
}
