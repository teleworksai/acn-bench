//! SPEC 090 §2: a turn's split, on hand-built rows with known answers
//! (ATR-10 to ATR-15, ATR-30).

#![allow(clippy::unwrap_used, clippy::expect_used, clippy::panic)] // CON-19: tests are exempt

use acn_attrib::core::{
    Decomposition, Hop, Leaf, LeafKind, LinkRow, Parts, TurnPath, TurnRow, decompose,
};

const S: [u8; 8] = [1; 8];

fn chat(id: u8, start: i64, end: i64) -> Leaf {
    Leaf {
        span_id: [id; 8],
        kind: LeafKind::Chat,
        start_ns: start,
        end_ns: end,
        placement: None,
    }
}

fn tool(id: u8, start: i64, end: i64, placement: &str) -> Leaf {
    Leaf {
        span_id: [id; 8],
        kind: LeafKind::Tool,
        start_ns: start,
        end_ns: end,
        placement: Some(placement.into()),
    }
}

fn path(start: i64, end: i64, leaves: Vec<Leaf>) -> TurnPath {
    TurnPath {
        session_id: S,
        turn_id: [9; 8],
        turn_index: 0,
        start_ns: start,
        end_ns: end,
        leaves,
    }
}

/// A link row of call `id`: sent at `e`, received at `q`, or lost when `q` is
/// `None`; its applied delay is its interval.
fn link(id: u8, dir: &str, e: i64, q: Option<i64>) -> LinkRow {
    LinkRow {
        call_id: Some([id; 8]),
        link_id: "p".into(),
        direction: dir.into(),
        enqueue_ns: e,
        dequeue_ns: q.unwrap_or(e),
        dropped: q.is_none(),
        applied_delay_ns: q.map_or(0, |q| q - e),
        rate_limited_ns: 0,
    }
}

/// The turn view's row for `p` as ADR-14 computes it from the same rows.
fn row(p: &TurnPath, links: &[LinkRow]) -> TurnRow {
    let (mut tool, mut chat, mut wait) = (0, 0, 0);
    for l in &p.leaves {
        let t = l.end_ns - l.start_ns;
        match l.kind {
            LeafKind::Tool => tool += t,
            LeafKind::Chat => {
                chat += t;
                for r in links.iter().filter(|r| r.call_id == Some(l.span_id)) {
                    wait += r.applied_delay_ns + r.rate_limited_ns;
                }
            }
        }
    }
    TurnRow {
        session_id: S,
        turn_index: 0,
        duration_ns: p.end_ns - p.start_ns,
        tool_wait_ns: tool,
        model_wait_ns: chat - wait,
        queue_wait_ns: None,
        stalls: 0,
        retries: 0,
    }
}

fn one(p: TurnPath, links: &[LinkRow]) -> Decomposition {
    let t = row(&p, links);
    let mut d = decompose(&[t], &[p], links).unwrap();
    let d = d.remove(0);
    let ps = d.parts;
    assert_eq!(
        ps.network_ns + ps.model_ns + ps.tool_ns + ps.retry_ns + ps.other_ns,
        ps.duration_ns,
        "the parts split the duration (ATR-10)"
    );
    assert_eq!(
        d.hop_ns.values().sum::<i64>(),
        ps.network_ns,
        "the hops split the network time (ATR-13)"
    );
    d
}

fn fails(p: TurnPath, links: &[LinkRow], needle: &str) {
    let t = row(&p, links);
    let e = decompose(&[t], &[p], links).unwrap_err();
    assert!(e.to_string().contains(needle), "{e}");
}

fn parts(d: &Decomposition) -> [i64; 5] {
    let p: Parts = d.parts;
    [p.network_ns, p.model_ns, p.tool_ns, p.retry_ns, p.other_ns]
}

const D: i64 = 40;

/// Cites: ATR-10, ATR-11
#[test]
fn a_plain_call_over_delay_d_has_2d_of_network_time() {
    let links = [
        link(1, "up", 0, Some(D)),
        link(1, "down", 600, Some(600 + D)),
    ];
    let d = one(path(0, 640, vec![chat(1, 0, 640)]), &links);
    assert_eq!(parts(&d), [2 * D, 560, 0, 0, 0]);
}

/// Cites: ATR-11
#[test]
fn a_streamed_answer_is_still_2d_however_its_messages_overlap() {
    let mut links = vec![link(1, "up", 0, Some(D))];
    // Events every 20 ns, closer than the delay: their intervals cover the decode.
    for k in 0..26 {
        let e = 100 + 20 * k;
        links.push(link(1, "down", e, Some(e + D)));
    }
    let d = one(path(0, 640, vec![chat(1, 0, 640)]), &links);
    assert_eq!(parts(&d), [2 * D, 560, 0, 0, 0]);
}

/// Cites: ATR-10, ATR-11
#[test]
fn client_time_before_the_request_and_after_the_answer_is_other() {
    let links = [
        link(1, "up", 50, Some(50 + D)),
        link(1, "down", 600, Some(600 + D)),
    ];
    let d = one(path(0, 700, vec![chat(1, 0, 700)]), &links);
    // 50 before the send, 60 after the receipt.
    assert_eq!(parts(&d), [2 * D, 510, 0, 0, 110]);
}

/// Cites: ATR-11
#[test]
fn a_lost_request_its_backoff_and_its_retry() {
    let links = [
        link(1, "up", 0, None),
        link(1, "up", 1000, Some(1000 + D)),
        link(1, "down", 1500, Some(1500 + D)),
    ];
    let d = one(path(0, 1540, vec![chat(1, 0, 1540)]), &links);
    // The failed attempt and its backoff are retry time.
    assert_eq!(parts(&d), [2 * D, 460, 0, 1000, 0]);
}

/// Cites: ATR-11
#[test]
fn a_request_lost_until_the_timeout_is_network_time() {
    let links = [link(1, "up", 0, None)];
    let d = one(path(0, 600, vec![chat(1, 0, 600)]), &links);
    assert_eq!(parts(&d), [600, 0, 0, 0, 0]);
}

/// Cites: ATR-11
#[test]
fn a_lost_last_message_is_network_time_until_the_call_ends() {
    let links = [link(1, "up", 0, Some(D)), link(1, "down", 500, None)];
    let d = one(path(0, 900, vec![chat(1, 0, 900)]), &links);
    assert_eq!(parts(&d), [D + 400, 460, 0, 0, 0]);
}

/// Cites: ATR-11
#[test]
fn an_unanswered_request_waits_on_the_server() {
    let links = [link(1, "up", 0, Some(D))];
    let d = one(path(0, 900, vec![chat(1, 0, 900)]), &links);
    assert_eq!(parts(&d), [D, 860, 0, 0, 0]);
}

/// Cites: ATR-11
#[test]
fn a_cut_stream_and_its_retry_only_the_last_attempt_is_split() {
    let links = [
        link(1, "up", 0, Some(D)),
        link(1, "down", 100, Some(100 + D)),
        link(1, "down", 120, None),
        link(1, "up", 300, Some(300 + D)),
        link(1, "down", 500, Some(500 + D)),
    ];
    let d = one(path(0, 540, vec![chat(1, 0, 540)]), &links);
    assert_eq!(parts(&d), [2 * D, 160, 0, 300, 0]);
}

/// Cites: ATR-11, ATR-14
#[test]
fn a_message_past_its_call_is_clipped_and_the_clipping_recorded() {
    let links = [link(1, "up", 0, Some(D)), link(1, "down", 500, Some(700))];
    let d = one(path(0, 600, vec![chat(1, 0, 600)]), &links);
    assert_eq!(parts(&d), [D + 100, 460, 0, 0, 0]);
    assert_eq!(d.clipped_ns, 100);
}

/// Cites: ATR-14
#[test]
fn link_time_with_no_call_is_reported_not_attributed() {
    let mut links = vec![
        link(1, "up", 0, Some(D)),
        link(1, "down", 500, Some(500 + D)),
    ];
    let mut stray = link(2, "down", 100, Some(300));
    stray.call_id = None;
    links.push(stray);
    let d = one(path(0, 540, vec![chat(1, 0, 540)]), &links);
    assert_eq!(parts(&d), [2 * D, 460, 0, 0, 0]);
    assert_eq!(d.unattributed_link_ns, 200);
}

/// Cites: ATR-12
#[test]
fn a_remote_tool_is_split_as_a_chat_and_a_local_tool_is_all_tool_time() {
    let links = [
        link(1, "up", 0, Some(D)),
        link(1, "down", 600, Some(600 + D)),
        link(2, "up", 700, Some(700 + D)),
        link(2, "down", 900, Some(900 + D)),
    ];
    let p = path(
        0,
        1240,
        vec![
            chat(1, 0, 640),
            tool(2, 700, 940, "remote"),
            tool(3, 1000, 1240, "local"),
        ],
    );
    let d = one(p, &links);
    // Gap 640..700 and 940..1000 is other; the remote tool's server time is tool.
    assert_eq!(parts(&d), [4 * D, 560, 160 + 240, 0, 120]);
}

/// Cites: ATR-12
#[test]
fn a_local_tool_with_link_rows_is_an_error() {
    let links = [link(3, "up", 0, Some(D))];
    fails(
        path(0, 100, vec![tool(3, 0, 100, "local")]),
        &links,
        "local tool",
    );
}

/// Cites: ATR-10
#[test]
fn a_turn_with_no_leaves_is_all_other_and_a_zero_length_turn_is_zero() {
    let d = one(path(0, 500, vec![]), &[]);
    assert_eq!(parts(&d), [0, 0, 0, 0, 500]);
    let d = one(path(7, 7, vec![chat(1, 7, 7)]), &[]);
    assert_eq!(parts(&d), [0, 0, 0, 0, 0]);
    // A call with no link rows is all model time: no emulated network.
    let d = one(path(0, 500, vec![chat(1, 100, 400)]), &[]);
    assert_eq!(parts(&d), [0, 300, 0, 0, 200]);
}

/// Cites: ATR-10, ATR-15
#[test]
fn overlapping_or_escaping_leaves_are_errors() {
    fails(
        path(0, 1000, vec![chat(1, 0, 600), chat(2, 500, 900)]),
        &[],
        "overlap",
    );
    fails(path(100, 1000, vec![chat(1, 50, 600)]), &[], "outside");
    fails(path(0, 1000, vec![chat(1, 500, 1100)]), &[], "outside");
}

/// Cites: ATR-11, ATR-15
#[test]
fn rows_that_cannot_happen_are_errors() {
    // An answer before any request.
    fails(
        path(0, 900, vec![chat(1, 0, 900)]),
        &[link(1, "down", 10, Some(20)), link(1, "up", 100, Some(140))],
        "before the first request",
    );
    // An answer sent before its request arrived.
    fails(
        path(0, 900, vec![chat(1, 0, 900)]),
        &[link(1, "up", 0, Some(100)), link(1, "down", 50, Some(150))],
        "before its request was received",
    );
    // An answer to a lost request.
    fails(
        path(0, 900, vec![chat(1, 0, 900)]),
        &[link(1, "up", 0, None), link(1, "down", 50, Some(150))],
        "answers a lost request",
    );
    // A receipt before its send.
    let mut bad = link(1, "up", 100, Some(140));
    bad.dequeue_ns = 90;
    fails(
        path(0, 900, vec![chat(1, 0, 900)]),
        &[bad],
        "received before",
    );
    // A lost message with an interval.
    let mut lost = link(1, "up", 100, None);
    lost.dequeue_ns = 180;
    fails(
        path(0, 900, vec![chat(1, 0, 900)]),
        &[lost],
        "dropped but not empty",
    );
    // Clipping does not hide an answer before the request: the order is the
    // recorded one (ATR-11).
    fails(
        path(0, 900, vec![chat(1, 10, 100)]),
        &[link(1, "down", 2, Some(5)), link(1, "up", 3, Some(4))],
        "before the first request",
    );
}

/// Cites: ATR-13
#[test]
fn network_time_is_split_by_hop_in_hop_order() {
    let mut up = link(1, "up", 0, Some(30));
    up.link_id = "radio".into();
    let mut down = link(1, "down", 500, Some(550));
    down.link_id = "core".into();
    let d = one(path(0, 550, vec![chat(1, 0, 550)]), &[up, down]);
    let hops: Vec<(String, i64)> = d.hop_ns.iter().map(|(h, ns)| (h.key(), *ns)).collect();
    assert_eq!(
        hops,
        [("core/down".to_owned(), 50), ("radio/up".to_owned(), 30)]
    );
    let lost = link(1, "up", 0, None);
    let d = one(path(0, 300, vec![chat(1, 0, 300)]), &[lost]);
    assert_eq!(
        d.hop_ns.get(&Hop {
            link_id: "p".into(),
            direction: "up".into()
        }),
        Some(&300),
        "a lost request's wait is its hop's"
    );
}

/// Cites: ATR-14, ATR-15
#[test]
fn a_turn_the_views_read_differently_fails_the_bundle() {
    let links = [
        link(1, "up", 0, Some(D)),
        link(1, "down", 600, Some(600 + D)),
    ];
    let p = path(0, 640, vec![chat(1, 0, 640)]);
    let mut t = row(&p, &links);
    t.model_wait_ns += 1;
    let e = decompose(std::slice::from_ref(&t), std::slice::from_ref(&p), &links).unwrap_err();
    assert!(e.to_string().contains("ATR-14"), "{e}");
    let mut t = row(&p, &links);
    t.tool_wait_ns = 5;
    assert!(decompose(&[t], std::slice::from_ref(&p), &links).is_err());
    // A good first turn does not save the bundle: the second turn's error is
    // the bundle's, and it names that turn.
    let good = row(&p, &links);
    let mut bad = good.clone();
    bad.turn_index = 1;
    bad.model_wait_ns += 1;
    let mut p2 = p.clone();
    p2.turn_index = 1;
    let e = decompose(&[good, bad], &[p, p2], &links)
        .unwrap_err()
        .to_string();
    assert!(
        e.contains("turn 1 of session") && e.contains("ATR-14"),
        "{e}"
    );
}

/// Cites: ATR-30
#[test]
fn a_sum_that_overflows_is_an_error_not_a_wrapped_number() {
    let p = path(i64::MIN, i64::MAX, vec![]);
    let t = TurnRow {
        session_id: S,
        duration_ns: 0,
        ..TurnRow::default()
    };
    let e = decompose(&[t], &[p], &[]).unwrap_err();
    assert!(e.to_string().contains("ATR-30"), "{e}");
}

/// Cites: ATR-11
#[test]
fn of_answers_sent_together_the_last_in_row_order_is_the_last_message() {
    let up = link(1, "up", 0, Some(D));
    let delivered = link(1, "down", 500, Some(600));
    let lost = link(1, "down", 500, None);
    // Delivered last in row order: its 100 ns are network, the rest other.
    let rows = [up.clone(), lost.clone(), delivered.clone()];
    let d = one(path(0, 700, vec![chat(1, 0, 700)]), &rows);
    assert_eq!(parts(&d), [D + 100, 460, 0, 0, 100]);
    // Lost last in row order: the wait from its send to the end is network.
    let d = one(path(0, 700, vec![chat(1, 0, 700)]), &[up, delivered, lost]);
    assert_eq!(parts(&d), [D + 200, 460, 0, 0, 0]);
}

/// Cites: ATR-11
#[test]
fn of_requests_sent_together_the_last_in_row_order_is_the_last_attempt() {
    // Two requests at once: the later in row order is the last attempt.
    let a = link(1, "up", 0, Some(10));
    let b = link(1, "up", 0, Some(30));
    let down = link(1, "down", 300, Some(300 + D));
    let rows = [a.clone(), b.clone(), down.clone()];
    let d = one(path(0, 340, vec![chat(1, 0, 340)]), &rows);
    assert_eq!(parts(&d), [30 + D, 270, 0, 0, 0]);
    let d = one(path(0, 340, vec![chat(1, 0, 340)]), &[b, a, down]);
    assert_eq!(parts(&d), [10 + D, 290, 0, 0, 0]);
}

/// Cites: ATR-11
#[test]
fn an_answered_attempt_then_an_unanswered_one_waits_on_the_server() {
    let links = [
        link(1, "up", 0, Some(10)),
        link(1, "down", 50, Some(60)),
        link(1, "up", 200, Some(210)),
    ];
    let d = one(path(0, 500, vec![chat(1, 0, 500)]), &links);
    // The first attempt and its backoff are retry time; the last waits on the server.
    assert_eq!(parts(&d), [10, 290, 0, 200, 0]);
}

/// Cites: ATR-11, ATR-14
#[test]
fn a_request_sent_before_its_call_started_is_clipped() {
    let links = [
        link(1, "up", -30, Some(D)),
        link(1, "down", 500, Some(500 + D)),
    ];
    let d = one(path(0, 540, vec![chat(1, 0, 540)]), &links);
    assert_eq!(parts(&d), [2 * D, 460, 0, 0, 0]);
    assert_eq!(d.clipped_ns, 30);
}

/// Cites: ATR-10, ATR-11
#[test]
fn a_zero_length_leaf_has_zero_parts_whatever_rows_it_carries() {
    let d = one(
        path(0, 100, vec![chat(1, 50, 50)]),
        &[link(1, "down", 50, Some(50))],
    );
    assert_eq!(parts(&d), [0, 0, 0, 0, 100]);
}

/// Cites: ATR-11, ATR-30
#[test]
fn row_order_matters_only_for_rows_sent_together() {
    let rows = vec![
        link(1, "up", 0, None),
        link(1, "up", 100, Some(130)),
        link(1, "down", 140, Some(150)),
        link(1, "up", 400, Some(420)),
        link(1, "down", 500, Some(520)),
        link(1, "down", 560, Some(600)),
        link(1, "down", 610, None),
    ];
    let p = path(0, 900, vec![chat(1, 0, 900)]);
    let want = one(p.clone(), &rows);
    // Every rotation and the reversal of rows with distinct send times agree.
    let mut perms: Vec<Vec<LinkRow>> = (0..rows.len())
        .map(|k| {
            let mut r = rows.clone();
            r.rotate_left(k);
            r
        })
        .collect();
    let mut rev = rows.clone();
    rev.reverse();
    perms.push(rev);
    for r in perms {
        assert_eq!(one(p.clone(), &r), want);
    }
}
