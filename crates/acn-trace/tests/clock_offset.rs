//! TRC-26: spans from a producer on another machine are put on the run's clock by
//! the ingester, using the call they answer, and the applied offset is recorded.

#![allow(clippy::unwrap_used, clippy::expect_used, clippy::panic)] // CON-19: tests are exempt

use acn_trace::ingest::{self, CLOCK_OFFSET};
use acn_trace::model::{AttrValue, Attrs, EventRow, ResourceRow, SpanRow, Trace, kind};

const T: [u8; 16] = [1; 16];
const SKEW: i64 = 10_000_000;

fn id(n: u8) -> [u8; 8] {
    [n, 0, 0, 0, 0, 0, 0, 0]
}

fn span(
    n: u8,
    parent: Option<u8>,
    name: &str,
    resource: i32,
    t: (i64, i64),
    a: &[(&str, AttrValue)],
) -> SpanRow {
    SpanRow {
        trace_id: T,
        span_id: id(n),
        parent_span_id: parent.map(id),
        name: name.into(),
        kind: kind::INTERNAL,
        start_ns: t.0,
        end_ns: t.1,
        status_code: 0,
        status_message: None,
        resource_id: resource,
        attrs: a
            .iter()
            .map(|(k, v)| ((*k).to_owned(), v.clone()))
            .collect(),
    }
}

fn link(n: u8, direction: &str, enqueue: i64, dequeue: i64) -> SpanRow {
    span(
        n,
        Some(0x10),
        "acn.link",
        0,
        (enqueue, dequeue),
        &[
            ("acn.link.direction", AttrValue::String(direction.into())),
            ("acn.link.enqueue_ns", AttrValue::Int(enqueue)),
            ("acn.link.dequeue_ns", AttrValue::Int(dequeue)),
        ],
    )
}

/// A call [1000, 5000] on the run's clock, answered by an inference node whose
/// request span ([1500, 4500] in truth) and prefill child ([2000, 3000]) carry the
/// node's clock, `skew` ahead.
fn trace(skew: i64, links: Vec<SpanRow>) -> Trace {
    let mut spans = vec![
        span(1, None, "acn.session", 0, (0, 9000), &[]),
        span(2, Some(1), "acn.turn", 0, (100, 8000), &[]),
        span(0x10, Some(2), "chat", 0, (1000, 5000), &[]),
        span(
            0x20,
            Some(0x10),
            "llm_request",
            1,
            (1500 + skew, 4500 + skew),
            &[],
        ),
        span(
            0x21,
            Some(0x20),
            "prefill",
            1,
            (2000 + skew, 3000 + skew),
            &[],
        ),
    ];
    spans.extend(links);
    let mut t = Trace {
        spans,
        events: vec![EventRow {
            trace_id: T,
            span_id: id(0x21),
            seq: 0,
            time_ns: 2500 + skew,
            name: "first_token".into(),
            attrs: Attrs::new(),
        }],
        links: Vec::new(),
        resources: vec![
            ResourceRow {
                resource_id: 0,
                attrs: [
                    (
                        "service.name".to_owned(),
                        AttrValue::String("acn-harness".into()),
                    ),
                    ("acn.engine_hash".to_owned(), AttrValue::String("e".into())),
                ]
                .into(),
            },
            ResourceRow {
                resource_id: 1,
                attrs: [("service.name".to_owned(), AttrValue::String("vllm".into()))].into(),
            },
        ],
    };
    t.sort();
    t
}

fn get(t: &Trace, n: u8) -> &SpanRow {
    t.spans.iter().find(|s| s.span_id == id(n)).unwrap()
}

/// Cites: TRC-26
#[test]
fn external_spans_are_centred_on_the_call_and_the_offset_is_recorded() {
    let aligned = ingest::align_clocks(&trace(SKEW, Vec::new())).unwrap();
    let root = get(&aligned, 0x20);
    assert_eq!((root.start_ns, root.end_ns), (1500, 4500));
    assert_eq!(root.attrs[CLOCK_OFFSET], AttrValue::Int(SKEW));
    let child = get(&aligned, 0x21);
    assert_eq!(
        (child.start_ns, child.end_ns),
        (2000, 3000),
        "the subtree moves with its root"
    );
    assert_eq!(child.attrs[CLOCK_OFFSET], AttrValue::Int(SKEW));
    assert_eq!(
        aligned.events[0].time_ns, 2500,
        "events move with their span"
    );
    // The run's own spans are untouched and carry no offset.
    for n in [1, 2, 0x10] {
        assert_eq!(get(&aligned, n), get(&trace(SKEW, Vec::new()), n));
        assert!(!get(&aligned, n).attrs.contains_key(CLOCK_OFFSET));
    }
}

/// Cites: TRC-26
#[test]
fn the_proxys_link_timestamps_are_the_reference_when_both_directions_exist() {
    // The request arrived at the last uplink dequeue (1200); the response left at
    // the last downlink enqueue (4900).
    let links = vec![
        link(0x30, "up", 1000, 1100),
        link(0x31, "up", 1050, 1200),
        link(0x32, "down", 4600, 4800),
        link(0x33, "down", 4900, 4950),
    ];
    let aligned = ingest::align_clocks(&trace(SKEW, links)).unwrap();
    // ((1500 + SKEW - 1200) + (4500 + SKEW - 4900)) / 2 = SKEW - 50
    assert_eq!(
        get(&aligned, 0x20).attrs[CLOCK_OFFSET],
        AttrValue::Int(SKEW - 50)
    );
    assert_eq!(get(&aligned, 0x20).start_ns, 1550);
    // With one direction only, the call's own start and end are the reference.
    let aligned = ingest::align_clocks(&trace(SKEW, vec![link(0x30, "up", 1000, 1100)])).unwrap();
    assert_eq!(
        get(&aligned, 0x20).attrs[CLOCK_OFFSET],
        AttrValue::Int(SKEW)
    );
}

/// Cites: TRC-26
#[test]
fn offsets_round_toward_negative_infinity_and_alignment_is_idempotent() {
    // A node one nanosecond and a half behind: ((-1) + (-2)) / 2 = -1.5 -> -2.
    let mut t = trace(0, Vec::new());
    for s in &mut t.spans {
        if s.span_id == id(0x20) {
            s.start_ns = 1499;
            s.end_ns = 4498;
        }
    }
    let once = ingest::align_clocks(&t).unwrap();
    assert_eq!(get(&once, 0x20).attrs[CLOCK_OFFSET], AttrValue::Int(-2));
    assert_eq!(get(&once, 0x20).start_ns, 1501);
    assert_eq!(
        ingest::align_clocks(&once).unwrap(),
        once,
        "a second pass changes nothing"
    );
}

/// Cites: TRC-26
#[test]
fn an_external_span_under_no_call_is_refused() {
    let mut t = trace(SKEW, Vec::new());
    for s in &mut t.spans {
        if s.span_id == id(0x20) {
            s.parent_span_id = Some(id(2)); // under the turn, not a call
        }
    }
    let err = ingest::align_clocks(&t).unwrap_err().to_string();
    assert!(err.contains("TRC-26"), "{err}");
}

/// Cites: TRC-26, TRC-15
#[test]
fn a_link_direction_outside_up_and_down_is_refused() {
    let err = ingest::align_clocks(&trace(SKEW, vec![link(0x30, "uplink", 1000, 1100)]))
        .unwrap_err()
        .to_string();
    assert!(err.contains("not `up` or `down`"), "{err}");
    // Downlink segments alone are not a reference: the call's bounds are.
    let aligned = ingest::align_clocks(&trace(SKEW, vec![link(0x32, "down", 4600, 4800)])).unwrap();
    assert_eq!(
        get(&aligned, 0x20).attrs[CLOCK_OFFSET],
        AttrValue::Int(SKEW)
    );
}

/// Cites: TRC-26
#[test]
fn all_roots_of_one_producer_under_one_call_share_one_offset() {
    // A retried request: two node spans under one call, on one machine clock.
    // Their envelope [1500, 4500] is centred on the call, and both shift by SKEW,
    // although each alone would centre differently.
    let mut t = trace(SKEW, Vec::new());
    for s in &mut t.spans {
        if s.span_id == id(0x20) {
            s.end_ns = 2500 + SKEW;
        }
    }
    t.spans.push(span(
        0x22,
        Some(0x10),
        "llm_request",
        1,
        (3000 + SKEW, 4500 + SKEW),
        &[],
    ));
    t.sort();
    let aligned = ingest::align_clocks(&t).unwrap();
    for n in [0x20, 0x22] {
        assert_eq!(
            get(&aligned, n).attrs[CLOCK_OFFSET],
            AttrValue::Int(SKEW),
            "root {n:#x}"
        );
    }
    assert_eq!(get(&aligned, 0x22).start_ns, 3000);
}

/// Cites: TRC-26
#[test]
fn an_external_span_under_a_local_span_that_is_not_a_call_is_refused() {
    // external root → local span → external span: the inner one hangs under no call.
    let mut t = trace(SKEW, Vec::new());
    t.spans
        .push(span(0x23, Some(0x20), "acn.marker", 0, (2000, 2100), &[]));
    t.spans.push(span(
        0x24,
        Some(0x23),
        "inner",
        1,
        (2000 + SKEW, 2050 + SKEW),
        &[],
    ));
    t.sort();
    assert!(
        ingest::align_clocks(&t)
            .unwrap_err()
            .to_string()
            .contains("TRC-26")
    );
}
