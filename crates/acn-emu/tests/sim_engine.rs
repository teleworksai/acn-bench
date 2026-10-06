//! The sim engine (SPEC 020 §4): the event queue's order and groups, the
//! network's arrival, response order and drops, and determinism on the
//! committed scenarios.
#![allow(clippy::unwrap_used, clippy::expect_used, clippy::panic)]

use std::path::{Path, PathBuf};

use acn_emu::link::{Delay, Direction, LinkSpec, OutageCause, OutageMode, Reorder, Window};
use acn_emu::scenario::{Scenario, load};
use acn_emu::sim::{CallOutcome, EventQueue, Network, Received};

const MS: i64 = 1_000_000;

fn synthetic(name: &str) -> PathBuf {
    Path::new(env!("CARGO_MANIFEST_DIR")).join(format!("../../scenarios/synthetic/{name}.toml"))
}

fn scenario(links: Vec<LinkSpec>) -> Scenario {
    Scenario {
        name: "t".into(),
        hash: String::new(),
        links,
    }
}

fn path(up: LinkSpec, down: LinkSpec) -> Network {
    Network::new(&scenario(vec![up, down]), 1).unwrap()
}

fn plain(direction: Direction) -> LinkSpec {
    LinkSpec::new("p", direction)
}

/// Cites: EMU-30, EMU-31
#[test]
fn the_queue_hands_out_groups_in_time_then_insertion_order() {
    let mut q = EventQueue::new();
    q.push(20, "c").unwrap();
    q.push(10, "a").unwrap();
    q.push(20, "d").unwrap();
    q.push(10, "b").unwrap();
    assert_eq!(q.next_time(), Some(10));
    assert_eq!(q.pop_group(), Some((10, vec!["a", "b"])));
    assert_eq!(q.now(), 10);
    // The past is refused; the present is not.
    assert_eq!(q.push(9, "x").unwrap_err().reason, "past");
    q.push(10, "e").unwrap();
    assert_eq!(q.pop_group(), Some((10, vec!["e"])));
    assert_eq!(q.pop_group(), Some((20, vec!["c", "d"])));
    assert_eq!(q.pop_group(), None);
    assert!(q.is_empty());
    // The clock does not move past a pending event.
    q.push(30, "f").unwrap();
    assert_eq!(q.advance_to(31).unwrap_err().reason, "past");
    q.advance_to(30).unwrap();
    assert_eq!(q.now(), 30);
}

/// Cites: EMU-32
#[test]
fn a_path_needs_both_directions() {
    let only_up = scenario(vec![plain(Direction::Up)]);
    assert_eq!(Network::new(&only_up, 1).unwrap_err().reason, "path");
    let only_down = scenario(vec![plain(Direction::Down)]);
    assert_eq!(Network::new(&only_down, 1).unwrap_err().reason, "path");
    let mut n = path(plain(Direction::Up), plain(Direction::Down));
    assert_eq!(n.paths().collect::<Vec<_>>(), vec!["p"]);
    assert_eq!(n.request(1, "q", 0, 1).unwrap_err().reason, "path");
}

/// Cites: EMU-32, EMU-33
#[test]
fn a_request_reaches_the_server_at_its_delivery_time() {
    let mut up = plain(Direction::Up);
    up.delay = Some(Delay {
        delay_ns: 25 * MS,
        jitter_ns: 0,
    });
    let mut n = path(up, plain(Direction::Down));
    let f = n.request(1, "p", 100, 500).unwrap();
    assert_eq!(f.outcome, Ok(100 + 25 * MS));
    // Two requests sent at one instant are delivered at one instant, in the
    // order sent.
    let a = n.request(2, "p", 200, 1).unwrap().outcome.unwrap();
    let b = n.request(3, "p", 200, 1).unwrap().outcome.unwrap();
    assert_eq!(a, b);
    assert_eq!(n.request(2, "p", 300, 1).unwrap_err().reason, "call");
}

/// Cites: EMU-32, EMU-1
#[test]
fn concurrent_responses_meet_the_downlink_in_time_order() {
    let mut n = path(plain(Direction::Up), plain(Direction::Down));
    n.request(1, "p", 0, 1).unwrap();
    n.request(2, "p", 0, 1).unwrap();
    n.respond(1, "p", &[(10, 5), (30, 5)]).unwrap();
    n.respond(2, "p", &[(20, 5), (30, 5)]).unwrap();
    assert_eq!(n.next_time(), Some(10));
    let got: Vec<(u64, usize, i64)> = n
        .advance(100)
        .unwrap()
        .iter()
        .map(|r| (r.call, r.index, r.fate.send_ns))
        .collect();
    // By send time; at 30, in the order the responses were registered.
    assert_eq!(got, vec![(1, 0, 10), (2, 0, 20), (1, 1, 30), (2, 1, 30)]);
    assert_eq!(n.outcome(1), Some(CallOutcome::Received(30)));
    // A message cannot be registered in the network's past.
    n.request(3, "p", 100, 1).unwrap();
    assert_eq!(n.respond(3, "p", &[(50, 1)]).unwrap_err().reason, "past");
    assert_eq!(n.respond(3, "p", &[]).unwrap_err().reason, "call");
}

/// Cites: EMU-34
#[test]
fn a_response_is_received_in_order() {
    // Every message is held back by a reorder gap on top of a jittered delay:
    // deliveries cross, receipts do not.
    let mut down = plain(Direction::Down);
    down.delay = Some(Delay {
        delay_ns: 10 * MS,
        jitter_ns: 9 * MS,
    });
    down.reorder = Some(Reorder {
        reorder_ppm: 1_000_000,
        gap_ns: MS,
    });
    let mut n = path(plain(Direction::Up), down);
    n.request(1, "p", 0, 1).unwrap();
    let msgs: Vec<(i64, u64)> = (0..200).map(|i| (i * MS, 10)).collect();
    n.respond(1, "p", &msgs).unwrap();
    let got: Vec<Received> = n.advance(i64::from(u32::MAX)).unwrap();
    let delivered: Vec<i64> = got.iter().map(|r| r.fate.outcome.unwrap()).collect();
    assert!(
        delivered.windows(2).any(|w| w[1] < w[0]),
        "no crossing to test"
    );
    let received: Vec<i64> = got.iter().map(|r| r.received_ns.unwrap()).collect();
    assert!(received.windows(2).all(|w| w[0] <= w[1]));
    assert!(
        got.iter()
            .all(|r| r.received_ns.unwrap() >= r.fate.outcome.unwrap())
    );
    assert_eq!(
        n.outcome(1),
        Some(CallOutcome::Received(*received.last().unwrap()))
    );
}

/// A down link that drops what is sent in `[100, 200)`.
fn outage_down() -> LinkSpec {
    let mut d = plain(Direction::Down);
    d.outage = Some(vec![Window {
        start_ns: 100,
        end_ns: 200,
        mode: OutageMode::Drop,
        cause: OutageCause::Scheduled,
    }]);
    d
}

/// Cites: EMU-35
#[test]
fn each_drop_ends_its_call_as_specified() {
    // A lost request: the call is lost whatever its response.
    let mut up = plain(Direction::Up);
    up.outage = Some(vec![Window {
        start_ns: 0,
        end_ns: 10,
        mode: OutageMode::Drop,
        cause: OutageCause::Handover,
    }]);
    let mut n = path(up, plain(Direction::Down));
    assert!(n.request(1, "p", 5, 1).unwrap().outcome.is_err());
    n.respond(1, "p", &[(20, 1)]).unwrap();
    n.advance(100).unwrap();
    assert_eq!(n.outcome(1), Some(CallOutcome::Lost));

    let mut n = path(plain(Direction::Up), outage_down());
    // A lost body.
    n.request(1, "p", 0, 1).unwrap();
    n.respond(1, "p", &[(150, 100)]).unwrap();
    // A stream that loses an event and receives a later one: cut there.
    n.request(2, "p", 0, 1).unwrap();
    n.respond(2, "p", &[(50, 1), (150, 1), (180, 1), (250, 1), (300, 1)])
        .unwrap();
    // A stream that loses its last events: lost.
    n.request(3, "p", 0, 1).unwrap();
    n.respond(3, "p", &[(60, 1), (160, 1)]).unwrap();
    // Not known before every message is carried.
    assert_eq!(n.outcome(2), None);
    n.advance(1_000).unwrap();
    assert_eq!(n.outcome(1), Some(CallOutcome::Lost));
    assert_eq!(
        n.outcome(2),
        Some(CallOutcome::Cut {
            first_lost: 1,
            at_ns: 250
        })
    );
    assert_eq!(n.outcome(3), Some(CallOutcome::Lost));
    assert_eq!(n.outcome(9), None);
}

/// A fixed exchange of calls over a scenario: (call, index, outcome) lines.
fn exchange(s: &Scenario, seed: u64) -> Vec<String> {
    let mut n = Network::new(s, seed).unwrap();
    let path = n.paths().next().unwrap().to_owned();
    let mut lines = Vec::new();
    for call in 0..12_u64 {
        let t = call.cast_signed() * 900 * MS;
        let arrive = n.request(call, &path, t, 2_000).unwrap().outcome;
        lines.push(format!("{call} up {arrive:?}"));
        if let Ok(at) = arrive {
            let msgs: Vec<(i64, u64)> = (0..4)
                .map(|k| (at + 50 * MS + k * 20 * MS, 1_400))
                .collect();
            n.respond(call, &path, &msgs).unwrap();
        }
        for r in n.advance(t + 900 * MS - 1).unwrap() {
            lines.push(format!("{} {} {:?}", r.call, r.index, r.received_ns));
        }
    }
    for r in n.advance(i64::MAX / 4).unwrap() {
        lines.push(format!("{} {} {:?}", r.call, r.index, r.received_ns));
    }
    lines
}

/// Cites: EMU-38
#[test]
fn the_same_seed_and_calls_give_the_same_network() {
    for name in ["cellular-handover", "5g-iana-replay", "clean"] {
        let s = load(&synthetic(name)).unwrap();
        assert_eq!(exchange(&s, 5), exchange(&s, 5), "{name}");
    }
    let s = load(&synthetic("cellular-handover")).unwrap();
    assert_ne!(exchange(&s, 5), exchange(&s, 6));
}

/// Cites: EMU-30, EMU-32, EMU-33, EMU-34, EMU-38
#[test]
fn golden_exchange_on_cellular_handover() {
    let s = load(&synthetic("cellular-handover")).unwrap();
    let got = exchange(&s, 42);
    assert_eq!(got, GOLDEN, "{got:#?}");
}

const GOLDEN: &[&str] = &[
    "0 up Ok(30302847)",
    "0 0 Some(104101285)",
    "0 1 Some(120982053)",
    "0 2 Some(146464379)",
    "0 3 Some(161611054)",
    "1 up Ok(929326066)",
    "1 0 Some(1005059888)",
    "1 1 Some(1019419060)",
    "1 2 Some(1040299027)",
    "1 3 Some(1069040919)",
    "2 up Ok(1820197659)",
    "2 0 Some(1891393124)",
    "2 1 Some(1916902953)",
    "2 2 Some(1938877004)",
    "2 3 Some(1959222929)",
    "3 up Ok(2720905824)",
    "3 0 Some(2791941365)",
    "3 1 Some(2818394128)",
    "3 2 Some(2839649094)",
    "3 3 Some(2856800764)",
    "4 up Ok(3623848431)",
    "4 0 Some(3695894455)",
    "4 1 Some(3723406479)",
    "4 2 Some(3735578150)",
    "4 3 Some(3760944725)",
    "5 up Ok(4528225575)",
    "5 0 Some(4600347945)",
    "5 1 Some(4625372000)",
    "5 2 Some(4641492354)",
    "5 3 Some(4663630234)",
    "6 up Ok(5424118576)",
    "6 0 Some(5500556706)",
    "6 1 Some(5521398857)",
    "6 2 Some(5539396378)",
    "6 3 Some(5561642585)",
    "7 up Ok(6326773525)",
    "7 0 Some(6403238556)",
    "7 1 Some(6424691964)",
    "7 2 Some(6445773272)",
    "7 3 None",
    "8 up Ok(7227445821)",
    "8 0 Some(7305841255)",
    "8 1 Some(7325066593)",
    "8 2 Some(7345599679)",
    "8 3 Some(7360719502)",
    "9 up Ok(8120714562)",
    "9 0 Some(8194257429)",
    "9 1 Some(8217503798)",
    "9 2 Some(8235955206)",
    "9 3 Some(8257561491)",
    "10 up Ok(9024652744)",
    "10 0 Some(9098850991)",
    "10 1 Some(9122379926)",
    "10 2 Some(9136211276)",
    "10 3 Some(9160921713)",
    "11 up Ok(9927469925)",
    "11 0 Some(10000195347)",
    "11 1 Some(10019899017)",
    "11 2 None",
    "11 3 None",
];
