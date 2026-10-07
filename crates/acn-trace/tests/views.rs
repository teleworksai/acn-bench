//! TRC-30..35, TRC-38: the derived views on a golden fixture with a known chain
//! length, fan-out, stalls and outages. The fixture is built row by row so that
//! every expected value below can be checked by hand; the critical-path readings
//! are those of ADR-14.
//!
//! Turn 0 (100..5000): C0 [200,1200] → tool X0 [1300,1800] → C1 [1900,3000] spawns
//! sub-agents A [3100,4500] (chat CA0 [3200,4400]) and A2 [3100,4000] (chat CB0
//! [3200,3900]) → C2 [4600,4900]. Its critical path is C0, X0, C1, CA0, C2: A ends
//! after A2, so CB0 and its 1000 ns link delay are off the path.
//! Turn 1 (6000..9000): C3 [6100,8000] → remote tool X1 [8100,8600].

#![allow(clippy::unwrap_used, clippy::expect_used, clippy::panic)] // CON-19: tests are exempt

use std::collections::BTreeMap;

use acn_trace::ingest;
use acn_trace::model::{AttrValue, Attrs, EventRow, LinkRow, ResourceRow, SpanRow, Trace, kind};
use acn_trace::schema;
use arrow_array::cast::AsArray as _;
use arrow_array::types::{Float64Type, Int64Type};
use arrow_array::{Array as _, RecordBatch};

const T: [u8; 16] = [1; 16];
const SC_TRACE: [u8; 16] = [2; 16];

fn id(n: u8) -> [u8; 8] {
    [n, 0, 0, 0, 0, 0, 0, 0]
}

fn s(v: &str) -> AttrValue {
    AttrValue::String(v.into())
}
fn i(v: i64) -> AttrValue {
    AttrValue::Int(v)
}
fn f(v: f64) -> AttrValue {
    AttrValue::Float(v)
}
fn b(v: bool) -> AttrValue {
    AttrValue::Bool(v)
}

fn attrs(kv: &[(&str, AttrValue)]) -> Attrs {
    kv.iter()
        .map(|(k, v)| ((*k).to_owned(), v.clone()))
        .collect()
}

fn span(
    tr: [u8; 16],
    n: u8,
    parent: Option<u8>,
    name: &str,
    span_kind: i8,
    (start, end): (i64, i64),
    a: &[(&str, AttrValue)],
) -> SpanRow {
    SpanRow {
        trace_id: tr,
        span_id: id(n),
        parent_span_id: parent.map(id),
        name: name.into(),
        kind: span_kind,
        start_ns: start,
        end_ns: end,
        status_code: 0,
        status_message: None,
        resource_id: 0,
        attrs: attrs(a),
    }
}

#[allow(clippy::too_many_arguments)]
fn chat(
    n: u8,
    parent: u8,
    t: (i64, i64),
    index: i64,
    streamed: bool,
    usage: Option<(i64, i64)>,
    wire: (i64, i64),
    retries: i64,
    extra: &[(&str, AttrValue)],
) -> SpanRow {
    let mut a = vec![
        ("gen_ai.operation.name", s("chat")),
        ("gen_ai.provider.name", s("mockllm")),
        ("gen_ai.request.model", s("m")),
        ("acn.call.index", i(index)),
        ("acn.call.new_input_tokens_method", s("tokens")),
        ("acn.call.wire_bytes_up", i(wire.0)),
        ("acn.call.wire_bytes_down", i(wire.1)),
        ("acn.call.streamed", b(streamed)),
        ("acn.call.retries", i(retries)),
    ];
    if let Some((input, cached)) = usage {
        a.push(("acn.call.input_tokens", i(input)));
        a.push(("acn.call.new_input_tokens", i(input - cached)));
        a.push(("acn.call.output_tokens", i(50)));
        a.push(("acn.cache.read_tokens", i(cached)));
        a.push(("acn.cache.write_tokens", i(0)));
    }
    a.extend(extra.iter().cloned());
    span(T, n, Some(parent), "chat", kind::CLIENT, t, &a)
}

fn link(n: u8, parent: u8, enqueue: i64, delay_ms: f64, rate_ms: f64) -> SpanRow {
    span(
        T,
        n,
        Some(parent),
        "acn.link",
        kind::INTERNAL,
        (enqueue, enqueue + 100),
        &[
            ("acn.link.id", s("uplink")),
            ("acn.link.model", s("ge")),
            ("acn.link.direction", s("up")),
            ("acn.link.bytes", i(1500)),
            ("acn.link.enqueue_ns", i(enqueue)),
            ("acn.link.dequeue_ns", i(enqueue + 100)),
            ("acn.link.applied_delay_ms", f(delay_ms)),
            ("acn.link.dropped", b(false)),
            ("acn.link.reordered", b(false)),
            ("acn.link.rate_limited_ms", f(rate_ms)),
        ],
    )
}

fn agent(n: u8, t: (i64, i64)) -> SpanRow {
    span(
        T,
        n,
        Some(2),
        "invoke_agent",
        kind::INTERNAL,
        t,
        &[
            ("acn.fanout.parent_turn", i(0)),
            ("acn.fanout.parent_call", i(1)),
            ("acn.fanout.depth", i(1)),
            ("acn.fanout.width", i(2)),
            ("acn.fanout.shared_prefix_tokens", i(500)),
        ],
    )
}

fn tool(n: u8, parent: u8, t: (i64, i64), class: &str, placement: &str, req: i64) -> SpanRow {
    span(
        T,
        n,
        Some(parent),
        "execute_tool",
        kind::INTERNAL,
        t,
        &[
            (
                "gen_ai.tool.name",
                s(if class == "file" {
                    "read_file"
                } else {
                    "fetch"
                }),
            ),
            ("acn.tool.class", s(class)),
            ("acn.tool.placement", s(placement)),
            ("acn.tool.requesting_call", i(req)),
            ("acn.tool.result_bytes", i(2048)),
        ],
    )
}

fn event(
    tr: [u8; 16],
    n: u8,
    seq: u32,
    time: i64,
    name: &str,
    a: &[(&str, AttrValue)],
) -> EventRow {
    EventRow {
        trace_id: tr,
        span_id: id(n),
        seq,
        time_ns: time,
        name: name.into(),
        attrs: attrs(a),
    }
}

fn golden() -> Trace {
    let session = span(
        T,
        1,
        None,
        "acn.session",
        kind::INTERNAL,
        (0, 10_000),
        &[
            ("acn.run_id", s("run")),
            ("acn.hypothesis.id", s("none")),
            ("acn.hypothesis.status", s("candidate")),
            ("acn.backend", s("mockllm")),
            ("acn.mode", s("sim")),
            ("acn.scenario.hash", s("sh")),
            ("acn.workload.hash", s("wh")),
            ("acn.seed", i(7)),
            ("acn.replicate", i(0)),
            ("acn.role", s("treatment")),
            ("acn.harness.knobs", s("{}")),
            ("acn.stall_threshold_ms", f(250.0)),
            ("acn.keep_content", b(false)),
        ],
    );
    let turn = |n: u8,
                t: (i64, i64),
                index: i64,
                outcome: &str,
                comp: &str,
                extra: &[(&str, AttrValue)]| {
        let mut a = vec![
            ("acn.turn.index", i(index)),
            ("acn.turn.outcome", s(outcome)),
            ("acn.turn.compaction", s(comp)),
        ];
        a.extend(extra.iter().cloned());
        span(T, n, Some(1), "acn.turn", kind::INTERNAL, t, &a)
    };
    let spans = vec![
        session,
        turn(2, (100, 5000), 0, "success", "none", &[]),
        turn(
            3,
            (6000, 9000),
            1,
            "failure",
            "window_full",
            &[
                ("acn.turn.deadline_ms", f(0.005)),
                ("acn.turn.first_useful_result_ms", f(0.0025)),
            ],
        ),
        chat(
            0x10,
            2,
            (200, 1200),
            0,
            true,
            Some((1000, 800)),
            (4000, 500),
            1,
            &[("acn.server.queue_ms", f(0.01))],
        ),
        link(0x50, 0x10, 250, 0.0001, 0.00005),
        link(0x51, 0x10, 900, 0.0002, 0.0),
        tool(0x11, 2, (1300, 1800), "file", "local", 0),
        chat(
            0x12,
            2,
            (1900, 3000),
            1,
            false,
            None,
            (100, 100),
            0,
            &[("acn.call.ttft_ms", f(1.1))],
        ),
        agent(0x20, (3100, 4500)),
        agent(0x21, (3100, 4000)),
        chat(
            0x30,
            0x20,
            (3200, 3800),
            0,
            false,
            Some((200, 0)),
            (10, 10),
            0,
            &[("acn.call.ttft_ms", f(0.6))],
        ),
        link(0x52, 0x30, 3300, 0.0003, 0.0),
        tool(0x22, 0x20, (3850, 4000), "file", "local", 0),
        // Not streamed and no token arrived: no ttft.
        chat(0x32, 0x20, (4050, 4400), 1, false, None, (10, 10), 0, &[]),
        chat(0x31, 0x21, (3200, 3900), 0, true, None, (10, 10), 0, &[]),
        link(0x53, 0x31, 3250, 0.001, 0.0),
        chat(
            0x13,
            2,
            (4600, 4900),
            2,
            true,
            Some((1400, 1200)),
            (1, 1),
            0,
            &[],
        ),
        // The first call of turn 1 is call 0 again (TRC-12).
        chat(
            0x14,
            3,
            (6100, 8000),
            0,
            true,
            Some((2000, 1800)),
            (1, 1),
            2,
            &[("acn.server.queue_ms", f(0.02))],
        ),
        tool(0x15, 3, (8100, 8600), "http", "remote", 0),
        link(0x54, 0x15, 8150, 0.0004, 0.0001),
        tool(0x16, 3, (8200, 8700), "file", "local", 0),
        chat(0x17, 3, (8700, 8900), 1, true, None, (1, 1), 0, &[]),
        span(
            SC_TRACE,
            0x40,
            None,
            "acn.scenario",
            kind::INTERNAL,
            (0, 10_000),
            &[
                ("acn.scenario.toml", s("[link]")),
                ("acn.scenario.hash", s("sh")),
            ],
        ),
    ];
    let events = vec![
        event(T, 0x10, 0, 300, "acn.stream.first_token", &[]),
        event(
            T,
            0x10,
            1,
            700,
            "acn.stream.stall",
            &[("gap_ms", f(0.3)), ("tokens_before", i(3))],
        ),
        event(T, 0x10, 2, 1150, "acn.stream.last_token", &[]),
        event(T, 0x13, 0, 4650, "acn.stream.first_token", &[]),
        event(T, 0x14, 0, 6200, "acn.stream.first_token", &[]),
        event(
            SC_TRACE,
            0x40,
            0,
            0,
            "acn.scenario.step",
            &[("step", s("s0")), ("params", s("{}"))],
        ),
        event(
            SC_TRACE,
            0x40,
            1,
            2000,
            "acn.scenario.step",
            &[("step", s("s1")), ("params", s("{}"))],
        ),
        event(
            SC_TRACE,
            0x40,
            2,
            3200,
            "acn.scenario.outage",
            &[
                ("start_ns", i(3200)),
                ("end_ns", i(3500)),
                ("cause", s("handover")),
            ],
        ),
        event(
            SC_TRACE,
            0x40,
            3,
            8000,
            "acn.scenario.outage",
            &[
                ("start_ns", i(8000)),
                ("end_ns", i(9000)),
                ("cause", s("scheduled")),
            ],
        ),
    ];
    let links = vec![LinkRow {
        trace_id: T,
        span_id: id(0x50),
        seq: 0,
        linked_trace_id: SC_TRACE,
        linked_span_id: id(0x40),
        attrs: Attrs::new(),
    }];
    let mut t = Trace {
        spans,
        events,
        links,
        resources: vec![ResourceRow {
            resource_id: 0,
            attrs: attrs(&[("service.name", s("acn-harness"))]),
        }],
    };
    t.sort();
    t
}

fn views() -> BTreeMap<String, RecordBatch> {
    let inv = schema::inventory().unwrap();
    ingest::views(&inv, &schema::views().unwrap(), &golden())
        .unwrap()
        .into_iter()
        .map(|(v, b)| (v.name, b))
        .collect()
}

fn ints(b: &RecordBatch, col: &str) -> Vec<Option<i64>> {
    let a = b.column_by_name(col).unwrap().as_primitive::<Int64Type>();
    (0..a.len())
        .map(|r| a.is_valid(r).then(|| a.value(r)))
        .collect()
}

fn strs(b: &RecordBatch, col: &str) -> Vec<Option<String>> {
    let a = b.column_by_name(col).unwrap().as_string::<i32>();
    (0..a.len())
        .map(|r| a.is_valid(r).then(|| a.value(r).to_owned()))
        .collect()
}

fn ids(b: &RecordBatch, col: &str) -> Vec<Option<u8>> {
    let a = b.column_by_name(col).unwrap().as_fixed_size_binary();
    (0..a.len())
        .map(|r| a.is_valid(r).then(|| a.value(r)[0]))
        .collect()
}

/// Cites: TRC-30, TRC-37
#[test]
fn every_view_has_exactly_the_columns_of_views_toml() {
    let declared = schema::views().unwrap();
    let got = views();
    for v in declared.iter() {
        let batch = &got[&v.name];
        let schema = batch.schema();
        let names: Vec<&str> = schema.fields().iter().map(|f| f.name().as_str()).collect();
        let expected: Vec<&str> = v.columns.iter().map(|c| c.name.as_str()).collect();
        assert_eq!(names, expected, "{}", v.name);
        for (field, col) in schema.fields().iter().zip(&v.columns) {
            assert_eq!(field.is_nullable(), col.nullable, "{}.{}", v.name, col.name);
        }
    }
    assert_eq!(got.len(), 5);
}

/// Cites: TRC-31
#[test]
fn the_session_view_totals_and_refuses_partial_sums() {
    let v = &views()["session"];
    assert_eq!(v.num_rows(), 1);
    assert_eq!(strs(v, "run_id"), [Some("run".into())]);
    assert_eq!(ids(v, "session_id"), [Some(1)]);
    assert_eq!(strs(v, "hypothesis_id"), [Some("none".into())]);
    assert_eq!(strs(v, "hypothesis_status"), [Some("candidate".into())]);
    assert_eq!(strs(v, "backend"), [Some("mockllm".into())]);
    assert_eq!(strs(v, "mode"), [Some("sim".into())]);
    assert_eq!(strs(v, "scenario_hash"), [Some("sh".into())]);
    assert_eq!(strs(v, "workload_hash"), [Some("wh".into())]);
    assert_eq!(ints(v, "seed"), [Some(7)]);
    assert_eq!(ints(v, "replicate"), [Some(0)]);
    assert_eq!(strs(v, "role"), [Some("treatment".into())]);
    assert_eq!(ints(v, "turns"), [Some(2)]);
    assert_eq!(ints(v, "calls"), [Some(8)]);
    assert_eq!(ints(v, "duration_ns"), [Some(10_000)]);
    assert_eq!(
        ints(v, "input_tokens_total"),
        [None],
        "C1, CB0, CA1 and C4 returned no usage"
    );
    assert_eq!(ints(v, "cache_read_tokens_total"), [None]);
    assert_eq!(ints(v, "calls_with_usage"), [Some(4)]);
    assert_eq!(ints(v, "wire_bytes_up_total"), [Some(4133)]);
    assert_eq!(ints(v, "wire_bytes_down_total"), [Some(633)]);
    let m = v.column_by_name("outcome_counts").unwrap().as_map();
    let keys = m.keys().as_string::<i32>();
    let vals = m.values().as_primitive::<Int64Type>();
    let counts: Vec<(&str, i64)> = (0..keys.len())
        .map(|e| (keys.value(e), vals.value(e)))
        .collect();
    assert_eq!(
        counts,
        [
            ("aborted", 0),
            ("failure", 1),
            ("success", 1),
            ("timeout", 0)
        ],
        "the closed set, zeros included, keys in bytewise order"
    );
}

/// Cites: TRC-32
#[test]
fn the_turn_view_walks_the_critical_path() {
    let v = &views()["turn"];
    assert_eq!(strs(v, "run_id"), [Some("run".into()), Some("run".into())]);
    assert_eq!(ids(v, "session_id"), [Some(1), Some(1)]);
    assert_eq!(ids(v, "turn_id"), [Some(2), Some(3)]);
    assert_eq!(ints(v, "turn_index"), [Some(0), Some(1)]);
    assert_eq!(
        ints(v, "chain_length"),
        [Some(3), Some(2)],
        "C0, C1, C2; C3, C4"
    );
    assert_eq!(ints(v, "fanout_width"), [Some(2), Some(0)]);
    assert_eq!(ints(v, "fanout_depth"), [Some(1), Some(0)]);
    assert_eq!(ints(v, "think_time_before_ns"), [None, Some(1000)]);
    assert_eq!(ints(v, "duration_ns"), [Some(4900), Some(3000)]);
    // C0's links (100 + 50 + 200) and CA0's (300); CB0's 1000 is off the path.
    // Turn 1: X1's remote link is off the path, because X2 ends later.
    assert_eq!(ints(v, "network_wait_ns"), [Some(650), Some(0)]);
    assert_eq!(
        ints(v, "tool_wait_ns"),
        [Some(650), Some(500)],
        "X0 + XA; X2"
    );
    // Chat time on the path (1000 + 1100 + 600 + 350 + 300; 1900 + 200) less the
    // chats' link wait.
    assert_eq!(ints(v, "model_wait_ns"), [Some(2700), Some(2100)]);
    assert_eq!(
        ints(v, "queue_wait_ns"),
        [None, None],
        "null when any chat on the path lacks it"
    );
    assert_eq!(ints(v, "stalls"), [Some(1), Some(0)]);
    assert_eq!(ints(v, "retries"), [Some(1), Some(2)]);
    assert_eq!(ints(v, "deadline_ns"), [None, Some(5000)]);
    assert_eq!(ints(v, "first_useful_result_ns"), [None, Some(2500)]);
    assert_eq!(
        strs(v, "outcome"),
        [Some("success".into()), Some("failure".into())]
    );
    assert_eq!(
        strs(v, "compaction"),
        [Some("none".into()), Some("window_full".into())]
    );
}

/// Cites: TRC-33
#[test]
fn the_call_view_finds_preceding_tools_by_cause_not_by_time() {
    let v = &views()["call"];
    // Rows in span start order: C0, C1, CA0 and CB0 (both at 3200, by id), CA1,
    // C2, C3, C4.
    assert_eq!(
        ids(v, "call_id"),
        [0x10, 0x12, 0x30, 0x31, 0x32, 0x13, 0x14, 0x17].map(Some)
    );
    assert_eq!(ints(v, "call_index"), [0, 1, 0, 0, 1, 2, 0, 1].map(Some));
    assert_eq!(ints(v, "turn_index"), [0, 0, 0, 0, 0, 0, 1, 1].map(Some));
    assert_eq!(
        ids(v, "lineage_id"),
        [
            None,
            None,
            Some(0x20),
            Some(0x21),
            Some(0x20),
            None,
            None,
            None
        ]
    );
    // By cause and by lineage: X0 and XA both answer call 0, but C1 consumed only
    // the main chain's X0 and CA1 only A's XA. C4 consumed the parallel X1 and X2:
    // latest end minus earliest start, and the class of the longest, ties broken
    // by the lowest span id (X1, http).
    assert_eq!(
        ints(v, "preceding_tool_count"),
        [None, Some(1), None, None, Some(1), None, None, Some(2)]
    );
    assert_eq!(
        ints(v, "preceding_tool_ns"),
        [
            None,
            Some(500),
            None,
            None,
            Some(150),
            None,
            None,
            Some(600)
        ]
    );
    assert_eq!(
        strs(v, "preceding_tool_class"),
        [
            None,
            Some("file"),
            None,
            None,
            Some("file"),
            None,
            None,
            Some("http")
        ]
        .map(|x| x.map(str::to_owned))
    );
    // Streamed: first token minus start; not streamed: the whole call when a token
    // arrived; no token: null.
    assert_eq!(
        ints(v, "ttft_ns"),
        [
            Some(100),
            Some(1100),
            Some(600),
            None,
            None,
            Some(50),
            Some(100),
            None
        ]
    );
    let ratio = v
        .column_by_name("cached_token_ratio")
        .unwrap()
        .as_primitive::<Float64Type>();
    let got: Vec<Option<f64>> = (0..ratio.len())
        .map(|r| ratio.is_valid(r).then(|| ratio.value(r)))
        .collect();
    assert_eq!(
        got,
        [
            Some(0.8),
            None,
            Some(0.0),
            None,
            None,
            Some(1200.0 / 1400.0),
            Some(0.9),
            None
        ]
    );
    assert_eq!(
        ints(v, "server_queue_ns"),
        [
            Some(10_000),
            None,
            None,
            None,
            None,
            None,
            Some(20_000),
            None
        ],
        "a _ms float becomes integer nanoseconds once, round-half-even"
    );
    assert_eq!(
        ints(v, "input_tokens"),
        [
            Some(1000),
            None,
            Some(200),
            None,
            None,
            Some(1400),
            Some(2000),
            None
        ]
    );
    assert_eq!(
        ints(v, "new_input_tokens"),
        [
            Some(200),
            None,
            Some(200),
            None,
            None,
            Some(200),
            Some(200),
            None
        ]
    );
    assert_eq!(
        ints(v, "output_tokens"),
        [
            Some(50),
            None,
            Some(50),
            None,
            None,
            Some(50),
            Some(50),
            None
        ]
    );
    assert_eq!(
        ints(v, "duration_ns"),
        [1000, 1100, 600, 700, 350, 300, 1900, 200].map(Some)
    );
    assert_eq!(
        ints(v, "wire_bytes_up"),
        [4000, 100, 10, 10, 10, 1, 1, 1].map(Some)
    );
    assert_eq!(ints(v, "retries"), [1, 0, 0, 0, 0, 0, 2, 0].map(Some));
    assert_eq!(
        strs(v, "provider"),
        vec![Some("mockllm".to_owned()); 8],
        "gen_ai.* read at ingest only"
    );
    assert_eq!(strs(v, "model"), vec![Some("m".to_owned()); 8]);
    assert_eq!(
        strs(v, "new_input_tokens_method"),
        vec![Some("tokens".to_owned()); 8]
    );
}

/// Cites: TRC-34
#[test]
fn the_link_view_places_each_segment_in_its_step_and_outage() {
    let v = &views()["link"];
    assert_eq!(
        ids(v, "link_span_id"),
        [0x50, 0x51, 0x53, 0x52, 0x54].map(Some)
    );
    assert_eq!(ids(v, "call_id"), [0x10, 0x10, 0x31, 0x30, 0x15].map(Some));
    assert_eq!(
        strs(v, "scenario_step"),
        ["s0", "s0", "s1", "s1", "s1"].map(|x| Some(x.to_owned()))
    );
    assert_eq!(
        ints(v, "outage_id"),
        [None, None, Some(0), Some(0), Some(1)]
    );
    assert_eq!(
        ints(v, "applied_delay_ns"),
        [100, 200, 1000, 300, 400].map(Some)
    );
    assert_eq!(ints(v, "rate_limited_ns"), [50, 0, 0, 0, 100].map(Some));
    assert_eq!(
        ints(v, "enqueue_ns"),
        [250, 900, 3250, 3300, 8150].map(Some)
    );
    assert_eq!(
        ints(v, "dequeue_ns"),
        [350, 1000, 3350, 3400, 8250].map(Some)
    );
    assert_eq!(ints(v, "bytes"), [Some(1500); 5]);
    assert_eq!(strs(v, "link_id"), vec![Some("uplink".to_owned()); 5]);
    assert_eq!(strs(v, "link_model"), vec![Some("ge".to_owned()); 5]);
    assert_eq!(strs(v, "direction"), vec![Some("up".to_owned()); 5]);
    assert_eq!(strs(v, "run_id"), vec![Some("run".to_owned()); 5]);
}

/// Cites: TRC-34
#[test]
fn step_and_outage_boundaries_follow_their_stated_rules() {
    let mut t = golden();
    let at = |t: &mut Trace, n: u8, enqueue: i64| {
        let l = t.spans.iter_mut().find(|s| s.span_id == id(n)).unwrap();
        l.attrs.insert("acn.link.enqueue_ns".into(), i(enqueue));
    };
    // A step takes effect at its own time; an outage ends before its end_ns.
    at(&mut t, 0x50, 2000);
    at(&mut t, 0x51, 3500);
    // A third outage overlapping the first: [3000, 3600) sorts first, and a
    // segment inside both gets the lowest index.
    t.events.push(event(
        SC_TRACE,
        0x40,
        4,
        3000,
        "acn.scenario.outage",
        &[
            ("start_ns", i(3000)),
            ("end_ns", i(3600)),
            ("cause", s("scheduled")),
        ],
    ));
    // Two steps at one instant: the one emitted later takes effect.
    t.events.push(event(
        SC_TRACE,
        0x40,
        5,
        2000,
        "acn.scenario.step",
        &[("step", s("a-late")), ("params", s("{}"))],
    ));
    t.sort();
    let inv = schema::inventory().unwrap();
    let all = ingest::views(&inv, &schema::views().unwrap(), &t).unwrap();
    let v = &all.iter().find(|(v, _)| v.name == "link").unwrap().1;
    // Rows keep their span start order; only the enqueue times moved.
    assert_eq!(
        ids(v, "link_span_id"),
        [0x50, 0x51, 0x53, 0x52, 0x54].map(Some)
    );
    assert_eq!(
        ints(v, "enqueue_ns"),
        [2000, 3500, 3250, 3300, 8150].map(Some)
    );
    assert_eq!(
        strs(v, "scenario_step"),
        ["a-late", "a-late", "a-late", "a-late", "a-late"].map(|x| Some(x.to_owned()))
    );
    // Outages sorted: [3000,3600)=0, [3200,3500)=1, [8000,9000)=2. 3500 is past the
    // end of outage 1 but inside outage 0; 3250 and 3300 are inside both.
    assert_eq!(
        ints(v, "outage_id"),
        [None, Some(0), Some(0), Some(0), Some(2)]
    );
}

/// Cites: TRC-38
#[test]
fn the_tool_view_takes_the_requesting_call_from_the_span() {
    let v = &views()["tool"];
    assert_eq!(ids(v, "tool_span_id"), [0x11, 0x22, 0x15, 0x16].map(Some));
    assert_eq!(ints(v, "requesting_call"), [Some(0); 4]);
    assert_eq!(ints(v, "turn_index"), [0, 0, 1, 1].map(Some));
    assert_eq!(
        strs(v, "tool_class"),
        ["file", "file", "http", "file"].map(|x| Some(x.to_owned()))
    );
    assert_eq!(
        strs(v, "placement"),
        ["local", "local", "remote", "local"].map(|x| Some(x.to_owned()))
    );
    assert_eq!(ints(v, "duration_ns"), [500, 150, 500, 500].map(Some));
    assert_eq!(
        ids(v, "lineage_id"),
        [None, Some(0x20), None, None],
        "XA runs in sub-agent A"
    );
    assert_eq!(
        strs(v, "tool_name"),
        ["read_file", "read_file", "fetch", "read_file"].map(|x| Some(x.to_owned()))
    );
    assert_eq!(ints(v, "result_bytes"), [Some(2048); 4]);
    assert_eq!(strs(v, "run_id"), vec![Some("run".to_owned()); 4]);
    assert_eq!(ids(v, "session_id"), [Some(1); 4]);
}

/// Cites: TRC-30
#[test]
fn views_are_deterministic_and_refuse_incomplete_spans() {
    let inv = schema::inventory().unwrap();
    let vs = schema::views().unwrap();
    let a = ingest::views(&inv, &vs, &golden()).unwrap();
    let b = ingest::views(&inv, &vs, &golden()).unwrap();
    for ((_, x), (_, y)) in a.iter().zip(&b) {
        assert_eq!(
            acn_trace::parquet_io::encode(x).unwrap(),
            acn_trace::parquet_io::encode(y).unwrap()
        );
    }
    let mut t = golden();
    t.spans
        .iter_mut()
        .find(|s| s.name == "chat")
        .unwrap()
        .attrs
        .remove("acn.call.retries");
    let err = ingest::views(&inv, &vs, &t).unwrap_err().to_string();
    assert!(err.contains("acn.call.retries"), "{err}");
    let mut t = golden();
    t.spans
        .iter_mut()
        .find(|s| s.name == "chat")
        .unwrap()
        .attrs
        .remove("gen_ai.provider.name");
    assert!(
        ingest::views(&inv, &vs, &t).is_err(),
        "a non-nullable column is never null"
    );
}

/// Cites: TRC-30
#[test]
fn milliseconds_become_nanoseconds_by_round_half_even() {
    assert_eq!(
        ingest::ms_to_ns(0.0000005).unwrap(),
        0,
        "0.5 ns rounds to even 0"
    );
    assert_eq!(
        ingest::ms_to_ns(0.0000015).unwrap(),
        2,
        "1.5 ns rounds to even 2"
    );
    assert_eq!(ingest::ms_to_ns(250.0).unwrap(), 250_000_000);
    assert!(ingest::ms_to_ns(f64::NAN).is_err());
    assert!(ingest::ms_to_ns(1e20).is_err());
}

/// Cites: TRC-30
#[test]
fn only_the_ingester_reads_convention_attributes() {
    // Producers may write `gen_ai.*` (the fixture does); nothing on the read side
    // but the ingester may name one. Provider response fields (normalise.rs) are
    // not span attributes.
    let crates = std::path::Path::new(env!("CARGO_MANIFEST_DIR")).join("..");
    // The schema names `gen_ai.input`/`gen_ai.output` only to forbid them as view
    // sources (TRC-42); that is a check, not a read.
    let allowed = [
        "acn-trace/src/ingest.rs",
        "acn-trace/src/fixture.rs",
        "acn-trace/src/schema/mod.rs",
        // A producer: it writes the GenAI attributes of its `chat` spans (HAR-31).
        "acn-harness/src/agent.rs",
        // A producer: the generator writes its tool and sub-agent spans'
        // GenAI attributes (SPEC 050 GEN-30).
        "acn-gen/src/sessions.rs",
    ];
    let mut offenders = Vec::new();
    for entry in walkdir::WalkDir::new(&crates) {
        let entry = entry.unwrap();
        let p = entry.path();
        let rel = p
            .strip_prefix(&crates)
            .unwrap()
            .to_string_lossy()
            .replace('\\', "/");
        if !rel.contains("/src/") || p.extension().is_none_or(|e| e != "rs") {
            continue;
        }
        if allowed.contains(&rel.as_str()) {
            continue;
        }
        let text = std::fs::read_to_string(p).unwrap();
        for conv in ["\"gen_ai.", "\"http.", "\"network."] {
            if text.contains(conv) {
                offenders.push(format!("{rel}: {conv}"));
            }
        }
    }
    assert!(offenders.is_empty(), "{offenders:?}");
}

/// The golden session and turn 0 with `children` as the turn's only work.
fn mini(children: Vec<SpanRow>) -> Trace {
    let g = golden();
    let mut spans: Vec<SpanRow> = g
        .spans
        .into_iter()
        .filter(|s| s.span_id == id(1) || s.span_id == id(2))
        .collect();
    spans.extend(children);
    let mut t = Trace {
        spans,
        events: Vec::new(),
        links: Vec::new(),
        resources: g.resources,
    };
    t.sort();
    t
}

fn turn_of(t: &Trace) -> RecordBatch {
    let inv = schema::inventory().unwrap();
    ingest::views(&inv, &schema::views().unwrap(), t)
        .unwrap()
        .into_iter()
        .find(|(v, _)| v.name == "turn")
        .unwrap()
        .1
}

/// Cites: TRC-32
#[test]
fn a_zero_length_span_keeps_its_predecessors_on_the_path_whatever_its_id() {
    // C3 [6100,7000] → zero-length tool Z [7000,7000] → C4 [7000,8000]. Z ties with
    // C3 at 7000; whichever id Z draws, C3 stays on the path.
    for z in [0x04, 0x7f] {
        let t = mini(vec![
            chat(0x40, 2, (6100, 7000), 0, true, None, (1, 1), 0, &[]),
            tool(z, 2, (7000, 7000), "file", "local", 0),
            chat(0x41, 2, (7000, 8000), 1, true, None, (1, 1), 0, &[]),
        ]);
        let v = turn_of(&t);
        assert_eq!(ints(&v, "chain_length"), [Some(2)], "Z id {z:#x}");
        assert_eq!(ints(&v, "model_wait_ns"), [Some(1900)], "Z id {z:#x}");
    }
}

/// Cites: TRC-32
#[test]
fn an_end_time_tie_goes_to_the_longer_span_not_the_lower_id() {
    // Chat P [0,1000] and tool Q [500,1000] end together before R. P started first,
    // so the turn waited on P: the tool is off the path whichever id is lower.
    for (p, q) in [(0x40, 0x41), (0x41, 0x40)] {
        let t = mini(vec![
            chat(p, 2, (100, 1000), 0, true, None, (1, 1), 0, &[]),
            tool(q, 2, (500, 1000), "file", "local", 0),
            chat(0x42, 2, (1000, 1200), 1, true, None, (1, 1), 0, &[]),
        ]);
        let v = turn_of(&t);
        assert_eq!(ints(&v, "tool_wait_ns"), [Some(0)], "P {p:#x}, Q {q:#x}");
        assert_eq!(ints(&v, "chain_length"), [Some(2)]);
        assert_eq!(ints(&v, "model_wait_ns"), [Some(1100)]);
    }
}

/// Cites: TRC-30
#[test]
fn views_do_not_depend_on_input_order() {
    let inv = schema::inventory().unwrap();
    let vs = schema::views().unwrap();
    let encode = |t: &Trace| -> Vec<Vec<u8>> {
        ingest::views(&inv, &vs, t)
            .unwrap()
            .iter()
            .map(|(_, b)| acn_trace::parquet_io::encode(b).unwrap())
            .collect()
    };
    let mut shuffled = golden();
    shuffled.spans.reverse();
    shuffled.events.reverse();
    shuffled.spans.rotate_left(7);
    shuffled.sort();
    assert_eq!(encode(&golden()), encode(&shuffled));
    // An empty run has empty views, not an error.
    for (_, b) in ingest::views(&inv, &vs, &Trace::default()).unwrap() {
        assert_eq!(b.num_rows(), 0);
    }
}

/// Cites: TRC-30, TRC-32, TRC-33, TRC-34
#[test]
fn the_ingester_refuses_traces_that_would_give_wrong_numbers() {
    let inv = schema::inventory().unwrap();
    let vs = schema::views().unwrap();
    type Edit = Box<dyn Fn(&mut Trace)>;
    let set = |n: u8, k: &'static str, v: AttrValue| -> Edit {
        Box::new(move |t: &mut Trace| {
            t.spans
                .iter_mut()
                .find(|s| s.span_id == id(n))
                .unwrap()
                .attrs
                .insert(k.into(), v.clone());
        })
    };
    let cases: Vec<(&str, Edit)> = vec![
        ("share an acn.turn.index", set(3, "acn.turn.index", i(0))),
        (
            "not in the closed set",
            set(2, "acn.turn.outcome", s("great")),
        ),
        ("disagree on acn.run_id", {
            Box::new(|t: &mut Trace| {
                let mut other = t.spans.iter().find(|s| s.span_id == id(1)).unwrap().clone();
                other.trace_id = [9; 16];
                other.attrs.insert("acn.run_id".into(), s("other"));
                t.spans.push(other);
            })
        }),
        ("acn.call.index 5", set(0x12, "acn.call.index", i(5))),
        (
            "requesting call 7",
            set(0x11, "acn.tool.requesting_call", i(7)),
        ),
        ("not an int", set(0x10, "acn.call.retries", s("one"))),
        ("negative", set(0x50, "acn.link.applied_delay_ms", f(-0.1))),
        ("child of `execute_tool`", {
            Box::new(|t: &mut Trace| {
                t.spans
                    .iter_mut()
                    .find(|s| s.span_id == id(0x54))
                    .unwrap()
                    .parent_span_id = Some(id(0x16));
            })
        }),
        ("ends before it starts", {
            Box::new(|t: &mut Trace| {
                t.spans
                    .iter_mut()
                    .find(|s| s.span_id == id(0x11))
                    .unwrap()
                    .end_ns = 1000;
            })
        }),
        ("cycle", {
            Box::new(|t: &mut Trace| {
                t.spans
                    .iter_mut()
                    .find(|s| s.span_id == id(2))
                    .unwrap()
                    .parent_span_id = Some(id(0x10));
            })
        }),
        ("not on a `chat`", {
            Box::new(|t: &mut Trace| {
                t.events.push(event(
                    T,
                    2,
                    0,
                    150,
                    "acn.stream.stall",
                    &[("gap_ms", f(0.3)), ("tokens_before", i(1))],
                ));
            })
        }),
        ("belongs to no span", {
            Box::new(|t: &mut Trace| {
                t.events
                    .push(event(T, 0x7e, 0, 150, "acn.stream.first_token", &[]));
            })
        }),
        ("before its call started", {
            Box::new(|t: &mut Trace| {
                t.events
                    .iter_mut()
                    .find(|e| e.span_id == id(0x10) && e.name == "acn.stream.first_token")
                    .unwrap()
                    .time_ns = 150;
            })
        }),
        ("exactly one acn.scenario", {
            Box::new(|t: &mut Trace| {
                t.spans.retain(|s| s.name != "acn.scenario");
                t.events.retain(|e| e.trace_id != SC_TRACE);
                t.links.clear();
            })
        }),
        ("ends (3000) before it starts", {
            Box::new(|t: &mut Trace| {
                t.events.push(event(
                    SC_TRACE,
                    0x40,
                    9,
                    3000,
                    "acn.scenario.outage",
                    &[
                        ("start_ns", i(3500)),
                        ("end_ns", i(3000)),
                        ("cause", s("trace")),
                    ],
                ));
            })
        }),
        ("lacks `step`", {
            Box::new(|t: &mut Trace| {
                t.events.push(event(
                    SC_TRACE,
                    0x40,
                    9,
                    10,
                    "acn.scenario.step",
                    &[("params", s("{}"))],
                ));
            })
        }),
    ];
    for (needle, edit) in cases {
        let mut t = golden();
        edit(&mut t);
        t.sort();
        let err = ingest::views(&inv, &vs, &t).unwrap_err().to_string();
        assert!(err.contains(needle), "expected `{needle}`, got: {err}");
    }
}

/// Cites: TRC-30
#[test]
fn nesting_deeper_than_the_bound_is_an_error_not_a_stack_overflow() {
    let mut children = Vec::new();
    // MAX_DEPTH sub-agents deep, built on distinct ids (two bytes of id space).
    let depth = ingest::MAX_DEPTH + 2;
    for k in 0..depth {
        let mut a = agent(0, (3100, 4500));
        let n = u16::try_from(k + 0x100).unwrap().to_be_bytes();
        a.span_id = [n[0], n[1], 0xaa, 0, 0, 0, 0, 0];
        a.parent_span_id = Some(if k == 0 {
            id(2)
        } else {
            children.last().map(|c: &SpanRow| c.span_id).unwrap()
        });
        children.push(a);
    }
    let t = mini(children);
    let inv = schema::inventory().unwrap();
    let err = ingest::views(&inv, &schema::views().unwrap(), &t)
        .unwrap_err()
        .to_string();
    assert!(err.contains("deeper than"), "{err}");
}
