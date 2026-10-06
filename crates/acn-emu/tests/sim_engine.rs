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
    assert_eq!(n.outcome(1), Some(CallOutcome::Lost));
    // The server never saw it, so no response may be registered for it.
    assert_eq!(n.respond(1, "p", &[(20, 1)]).unwrap_err().reason, "call");

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

/// Cites: EMU-32
#[test]
fn a_bad_response_is_refused_whole() {
    let mut n = path(plain(Direction::Up), plain(Direction::Down));
    assert_eq!(n.respond(1, "p", &[(0, 1)]).unwrap_err().reason, "call");
    n.request(1, "p", 0, 1).unwrap();
    n.advance(50).unwrap();
    // Decreasing send times, a start before the clock, an end beyond 2^62:
    // refused, and nothing registered.
    assert_eq!(
        n.respond(1, "p", &[(60, 1), (55, 1)]).unwrap_err().reason,
        "call"
    );
    assert_eq!(
        n.respond(1, "p", &[(40, 1), (60, 1)]).unwrap_err().reason,
        "past"
    );
    assert_eq!(
        n.respond(1, "p", &[(60, 1), (1 << 62 | 1, 1)])
            .unwrap_err()
            .reason,
        "range"
    );
    assert_eq!(n.next_time(), None);
    assert_eq!(n.messages(1), None);
    n.respond(1, "p", &[(60, 1), (70, 1)]).unwrap();
    assert_eq!(n.respond(1, "p", &[(80, 1)]).unwrap_err().reason, "call");
    assert!(!n.forget(1), "a call with messages to carry is kept");
    let got = n.advance(100).unwrap();
    assert_eq!(got.len(), 2);
    assert_eq!(n.messages(1).unwrap().iter().flatten().count(), 2);
    assert_eq!(n.request_fate(1).unwrap().outcome, Ok(0));
    assert!(n.forget(1));
    assert_eq!(n.outcome(1), None);
    assert_eq!(n.advance(1 << 62 | 1).unwrap_err().reason, "range");
}

/// Cites: EMU-32
#[test]
fn a_path_name_and_direction_is_unique_and_its_links_are_reachable() {
    let twice = scenario(vec![
        plain(Direction::Up),
        plain(Direction::Up),
        plain(Direction::Down),
    ]);
    assert_eq!(Network::new(&twice, 1).unwrap_err().reason, "path");
    let n = path(plain(Direction::Up), plain(Direction::Down));
    assert_eq!(
        n.link("p", Direction::Down).unwrap().spec().direction,
        Direction::Down
    );
    assert!(n.link("q", Direction::Up).is_none());
}

/// Cites: EMU-32, EMU-7
#[test]
fn requests_sent_at_different_times_can_arrive_at_one_instant() {
    let mut up = plain(Direction::Up);
    up.outage = Some(vec![Window {
        start_ns: 100,
        end_ns: 200,
        mode: OutageMode::Hold,
        cause: OutageCause::Handover,
    }]);
    let mut n = path(up, plain(Direction::Down));
    assert_eq!(n.request(1, "p", 120, 1).unwrap().outcome, Ok(200));
    assert_eq!(n.request(2, "p", 180, 1).unwrap().outcome, Ok(200));
    assert_eq!(n.request(3, "p", 200, 1).unwrap().outcome, Ok(200));
}

/// A fixed exchange of calls over a scenario: (call, index, outcome) lines.
fn exchange(s: &Scenario, seed: u64) -> Vec<String> {
    let mut n = Network::new(s, seed).unwrap();
    let path = n.paths().next().unwrap().to_owned();
    let mut lines = Vec::new();
    let carried = |n: &mut Network, until: i64, lines: &mut Vec<String>| {
        for r in n.advance(until).unwrap() {
            lines.push(format!(
                "{} {} sent {} {:?} rx {:?}",
                r.call, r.index, r.fate.send_ns, r.fate.outcome, r.received_ns
            ));
        }
    };
    // Calls every 900 ms, and one more at 10.1 s, inside the handover (held
    // up, dropped down).
    let mut times: Vec<i64> = (0..12).map(|c| c * 900 * MS).collect();
    times.push(10_100 * MS);
    times.sort_unstable();
    for (call, t) in times.iter().enumerate() {
        let call = call as u64;
        carried(&mut n, *t, &mut lines);
        let f = n.request(call, &path, *t, 2_000).unwrap();
        lines.push(format!("{call} up {t} {:?} hold {}", f.outcome, f.hold_ns));
        if let Ok(at) = f.outcome {
            let msgs: Vec<(i64, u64)> = (0..4)
                .map(|k| (at + 50 * MS + k * 20 * MS, 1_400))
                .collect();
            n.respond(call, &path, &msgs).unwrap();
        }
    }
    carried(&mut n, 1 << 61, &mut lines);
    for call in 0..times.len() as u64 {
        lines.push(format!("{call} {:?}", n.outcome(call)));
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
    "0 up 0 Ok(30302847) hold 0",
    "0 0 sent 80302847 Ok(104101285) rx Some(104101285)",
    "0 1 sent 100302847 Ok(120982053) rx Some(120982053)",
    "0 2 sent 120302847 Ok(146464379) rx Some(146464379)",
    "0 3 sent 140302847 Ok(161611054) rx Some(161611054)",
    "1 up 900000000 Ok(929326066) hold 0",
    "1 0 sent 979326066 Ok(1005059888) rx Some(1005059888)",
    "1 1 sent 999326066 Ok(1019419060) rx Some(1019419060)",
    "1 2 sent 1019326066 Ok(1040299027) rx Some(1040299027)",
    "1 3 sent 1039326066 Ok(1069040919) rx Some(1069040919)",
    "2 up 1800000000 Ok(1820197659) hold 0",
    "2 0 sent 1870197659 Ok(1891393124) rx Some(1891393124)",
    "2 1 sent 1890197659 Ok(1916902953) rx Some(1916902953)",
    "2 2 sent 1910197659 Ok(1938877004) rx Some(1938877004)",
    "2 3 sent 1930197659 Ok(1959222929) rx Some(1959222929)",
    "3 up 2700000000 Ok(2720905824) hold 0",
    "3 0 sent 2770905824 Ok(2791941365) rx Some(2791941365)",
    "3 1 sent 2790905824 Ok(2818394128) rx Some(2818394128)",
    "3 2 sent 2810905824 Ok(2839649094) rx Some(2839649094)",
    "3 3 sent 2830905824 Ok(2856800764) rx Some(2856800764)",
    "4 up 3600000000 Ok(3623848431) hold 0",
    "4 0 sent 3673848431 Ok(3695894455) rx Some(3695894455)",
    "4 1 sent 3693848431 Ok(3723406479) rx Some(3723406479)",
    "4 2 sent 3713848431 Ok(3735578150) rx Some(3735578150)",
    "4 3 sent 3733848431 Ok(3760944725) rx Some(3760944725)",
    "5 up 4500000000 Ok(4528225575) hold 0",
    "5 0 sent 4578225575 Ok(4600347945) rx Some(4600347945)",
    "5 1 sent 4598225575 Ok(4625372000) rx Some(4625372000)",
    "5 2 sent 4618225575 Ok(4641492354) rx Some(4641492354)",
    "5 3 sent 4638225575 Ok(4663630234) rx Some(4663630234)",
    "6 up 5400000000 Ok(5424118576) hold 0",
    "6 0 sent 5474118576 Ok(5500556706) rx Some(5500556706)",
    "6 1 sent 5494118576 Ok(5521398857) rx Some(5521398857)",
    "6 2 sent 5514118576 Ok(5539396378) rx Some(5539396378)",
    "6 3 sent 5534118576 Ok(5561642585) rx Some(5561642585)",
    "7 up 6300000000 Ok(6326773525) hold 0",
    "7 0 sent 6376773525 Ok(6403238556) rx Some(6403238556)",
    "7 1 sent 6396773525 Ok(6424691964) rx Some(6424691964)",
    "7 2 sent 6416773525 Ok(6445773272) rx Some(6445773272)",
    "7 3 sent 6436773525 Err(Loss) rx None",
    "8 up 7200000000 Ok(7227445821) hold 0",
    "8 0 sent 7277445821 Ok(7305841255) rx Some(7305841255)",
    "8 1 sent 7297445821 Ok(7325066593) rx Some(7325066593)",
    "8 2 sent 7317445821 Ok(7345599679) rx Some(7345599679)",
    "8 3 sent 7337445821 Ok(7360719502) rx Some(7360719502)",
    "9 up 8100000000 Ok(8120714562) hold 0",
    "9 0 sent 8170714562 Ok(8194257429) rx Some(8194257429)",
    "9 1 sent 8190714562 Ok(8217503798) rx Some(8217503798)",
    "9 2 sent 8210714562 Ok(8235955206) rx Some(8235955206)",
    "9 3 sent 8230714562 Ok(8257561491) rx Some(8257561491)",
    "10 up 9000000000 Ok(9024652744) hold 0",
    "10 0 sent 9074652744 Ok(9098850991) rx Some(9098850991)",
    "10 1 sent 9094652744 Ok(9122379926) rx Some(9122379926)",
    "10 2 sent 9114652744 Ok(9136211276) rx Some(9136211276)",
    "10 3 sent 9134652744 Ok(9160921713) rx Some(9160921713)",
    "11 up 9900000000 Ok(9927469925) hold 0",
    "11 0 sent 9977469925 Ok(10000195347) rx Some(10000195347)",
    "11 1 sent 9997469925 Ok(10019899017) rx Some(10019899017)",
    "11 2 sent 10017469925 Err(Outage) rx None",
    "11 3 sent 10037469925 Err(Outage) rx None",
    "12 up 10100000000 Ok(10330019281) hold 200000000",
    "12 0 sent 10380019281 Ok(10409992735) rx Some(10409992735)",
    "12 1 sent 10400019281 Ok(10427106525) rx Some(10427106525)",
    "12 2 sent 10420019281 Ok(10441662932) rx Some(10441662932)",
    "12 3 sent 10440019281 Ok(10461931051) rx Some(10461931051)",
    "0 Some(Received(161611054))",
    "1 Some(Received(1069040919))",
    "2 Some(Received(1959222929))",
    "3 Some(Received(2856800764))",
    "4 Some(Received(3760944725))",
    "5 Some(Received(4663630234))",
    "6 Some(Received(5561642585))",
    "7 Some(Lost)",
    "8 Some(Received(7360719502))",
    "9 Some(Received(8257561491))",
    "10 Some(Received(9160921713))",
    "11 Some(Lost)",
    "12 Some(Received(10461931051))",
];
