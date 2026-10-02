//! TRC-28: OTLP/JSON export and import are inverse, and import refuses what the
//! model cannot hold faithfully.

#![allow(clippy::unwrap_used, clippy::expect_used, clippy::panic)] // CON-19: tests are exempt

use acn_trace::fixture::{self, FixtureRun};
use acn_trace::identity::Digest;
use acn_trace::model::{AttrValue, LinkRow, Trace, status};
use acn_trace::otlp;
use serde_json::{Value, json};

fn trace() -> Trace {
    let mut t = fixture::session(&FixtureRun {
        run_id: "r".into(),
        seed: 5,
        replicate: 0,
        engine_hash: Digest::of(b"e"),
        build_hash: Digest::of(b"b"),
    })
    .unwrap();
    let s0 = t.spans[0].clone();
    let chat = t.spans.iter_mut().find(|s| s.name == "chat").unwrap();
    chat.attrs
        .insert("x.blob".into(), AttrValue::Bytes(vec![0, 1, 2, 250]));
    chat.status_code = status::ERROR;
    chat.status_message = Some("upstream timeout".into());
    t.links.push(LinkRow {
        trace_id: s0.trace_id,
        span_id: s0.span_id,
        seq: 0,
        linked_trace_id: [7; 16],
        linked_span_id: [7; 8],
        attrs: [("w".to_owned(), AttrValue::Float(0.25))].into(),
    });
    t.sort();
    t
}

/// Cites: TRC-28
#[test]
fn export_then_import_returns_the_same_trace() {
    let t = trace();
    let doc = otlp::to_json(&t).unwrap();
    // Through text, as a collector would see it.
    let text = serde_json::to_string(&doc).unwrap();
    let back = otlp::from_json(&serde_json::from_str(&text).unwrap()).unwrap();
    assert_eq!(back, t);
}

/// Cites: TRC-28
#[test]
fn the_document_follows_the_otlp_json_encoding() {
    let doc = otlp::to_json(&trace()).unwrap();
    let rs = &doc["resourceSpans"][0];
    assert_eq!(rs["scopeSpans"][0]["scope"]["name"], otlp::SCOPE);
    let span = &rs["scopeSpans"][0]["spans"][0];
    let tid = span["traceId"].as_str().unwrap();
    assert_eq!(tid.len(), 32, "ids are lowercase hex, not base64");
    assert!(
        tid.bytes()
            .all(|b| b.is_ascii_digit() || (b'a'..=b'f').contains(&b))
    );
    assert!(
        span["startTimeUnixNano"].is_string(),
        "64-bit integers are strings"
    );
    assert!(span["kind"].is_number(), "enums are integers");
    let chat = rs["scopeSpans"][0]["spans"]
        .as_array()
        .unwrap()
        .iter()
        .find(|s| s["name"] == "chat")
        .unwrap();
    let attr = |k: &str| {
        chat["attributes"]
            .as_array()
            .unwrap()
            .iter()
            .find(|kv| kv["key"] == k)
            .unwrap()["value"]
            .clone()
    };
    assert_eq!(attr("acn.call.retries"), json!({ "intValue": "0" }));
    assert_eq!(attr("x.blob"), json!({ "bytesValue": "AAEC+g==" }));
    assert_eq!(attr("acn.call.streamed"), json!({ "boolValue": true }));
    assert_eq!(
        chat["status"],
        json!({ "code": 2, "message": "upstream timeout" })
    );
}

/// Cites: TRC-28
#[test]
fn import_resources_are_numbered_by_their_attributes_not_their_order() {
    let mut doc = otlp::to_json(&trace()).unwrap();
    let extra = json!({
        "resource": { "attributes": [{ "key": "service.name", "value": { "stringValue": "aaa-node" } }] },
        "scopeSpans": [{ "spans": [{
            "traceId": "0101010101010101010101010101010a", "spanId": "0a0a0a0a0a0a0a0a",
            "name": "x", "startTimeUnixNano": "1", "endTimeUnixNano": "2"
        }] }]
    });
    let rs = doc["resourceSpans"].as_array_mut().unwrap();
    rs.push(extra.clone());
    let a = otlp::from_json(&doc).unwrap();
    let rs = doc["resourceSpans"].as_array_mut().unwrap();
    rs.rotate_right(1);
    let b = otlp::from_json(&doc).unwrap();
    assert_eq!(a, b);
    assert_eq!(a.resources.len(), 2);
}

fn span_path(d: &mut Value) -> &mut Value {
    &mut d["resourceSpans"][0]["scopeSpans"][0]["spans"][0]
}

/// Cites: TRC-28, TRC-25
#[test]
fn import_refuses_what_the_model_cannot_hold() {
    let base = otlp::to_json(&trace()).unwrap();
    type Edit = Box<dyn Fn(&mut Value)>;
    let cases: Vec<(&str, Edit)> = vec![
        (
            "no member",
            Box::new(|d| {
                span_path(d)["attributes"][0]["value"] = json!({ "arrayValue": { "values": [] } });
            }),
        ),
        (
            "dropped",
            Box::new(|d| span_path(d)["droppedAttributesCount"] = json!(3)),
        ),
        (
            "hex",
            Box::new(|d| span_path(d)["spanId"] = json!("AQIDBAUGBwg=")),
        ),
        (
            "finite",
            Box::new(|d| {
                span_path(d)["attributes"][0]["value"] = json!({ "doubleValue": "NaN" });
            }),
        ),
        (
            "set twice",
            Box::new(|d| {
                let a = span_path(d)["attributes"][0].clone();
                span_path(d)["attributes"].as_array_mut().unwrap().push(a);
            }),
        ),
        (
            "int64",
            Box::new(|d| span_path(d)["startTimeUnixNano"] = json!("soon")),
        ),
        (
            "exactly one",
            Box::new(|d| {
                span_path(d)["attributes"][0]["value"] =
                    json!({ "stringValue": "a", "intValue": "1" });
            }),
        ),
    ];
    for (needle, edit) in cases {
        let mut d = base.clone();
        edit(&mut d);
        let err = otlp::from_json(&d).unwrap_err().to_string();
        assert!(err.contains(needle), "expected `{needle}`, got: {err}");
    }
}

/// Cites: TRC-28
#[test]
fn a_document_shaped_like_an_sdks_imports_field_by_field() {
    // Hand-written in the shape OTel SDKs and the collector emit: extra fields
    // (flags, traceState, schemaUrl, scope.version, zero dropped counts), enum
    // names, an int64 as a JSON number, a double as a string, and proto3's omitted
    // defaults (a span with no name, kind, start or status; an event with no time).
    let doc: Value = serde_json::from_str(include_str!("fixtures/otlp/sdk.json")).unwrap();
    let t = otlp::from_json(&doc).unwrap();
    assert_eq!(t.resources.len(), 1);
    assert_eq!(t.resources[0].attrs["process.pid"], AttrValue::Int(4242));
    let s = t.spans.iter().find(|s| s.name == "llm_request").unwrap();
    assert_eq!(s.trace_id[..4], [0x5b, 0x8e, 0xff, 0xf7]);
    assert_eq!(s.span_id, [0x05, 0x1f, 0x2e, 0x3d, 0x4c, 0x5b, 0x6a, 0x79]);
    assert_eq!(
        s.parent_span_id,
        Some([0xee, 0xe1, 0x9b, 0x7e, 0xc3, 0xc1, 0xb1, 0x74])
    );
    assert_eq!(s.kind, 2, "SPAN_KIND_SERVER");
    assert_eq!(
        (s.start_ns, s.end_ns),
        (1_544_712_660_000_000_000, 1_544_712_661_000_000_000)
    );
    assert_eq!(s.status_code, status::ERROR);
    assert_eq!(s.status_message.as_deref(), Some("upstream"));
    assert_eq!(
        s.attrs["gen_ai.latency.time_in_queue"],
        AttrValue::Float(0.25)
    );
    assert_eq!(s.attrs["gen_ai.usage.prompt_tokens"], AttrValue::Int(512));
    assert_eq!(
        s.attrs["x.blob"],
        AttrValue::Bytes(vec![0xde, 0xad, 0xbe, 0xef])
    );
    assert_eq!(s.attrs["x.flag"], AttrValue::Bool(false));
    let ev: Vec<_> = t.events.iter().filter(|e| e.span_id == s.span_id).collect();
    assert_eq!(ev.len(), 2);
    assert!(
        ev.iter()
            .any(|e| e.name == "zero-time event" && e.time_ns == 0 && e.seq == 1)
    );
    let bare = t
        .spans
        .iter()
        .find(|s| s.span_id == [10, 11, 12, 13, 14, 15, 16, 17])
        .unwrap();
    assert_eq!(
        (bare.name.as_str(), bare.kind, bare.start_ns, bare.end_ns),
        ("", 0, 0, 5)
    );
    assert_eq!(bare.parent_span_id, None, "an empty parentSpanId is a root");
    assert_eq!(
        (bare.status_code, bare.status_message.as_deref()),
        (0, None)
    );
}

/// Cites: TRC-28
#[test]
fn enums_out_of_range_and_array_values_are_refused() {
    let base = otlp::to_json(&trace()).unwrap();
    for (field, value) in [
        ("kind", json!(9)),
        ("kind", json!(-1)),
        ("kind", json!("SPAN_KIND_BOGUS")),
    ] {
        let mut d = base.clone();
        span_path(&mut d)[field] = value.clone();
        assert!(otlp::from_json(&d).is_err(), "{field} = {value}");
    }
    let mut d = base.clone();
    span_path(&mut d)["status"] = json!({ "code": 7 });
    assert!(otlp::from_json(&d).is_err(), "status code 7");
    // GenAI conventions emit string arrays (gen_ai.response.finish_reasons); the
    // profile stores scalars only, so they are refused, never flattened (ADR-15).
    let mut d = base;
    span_path(&mut d)["attributes"][0]["value"] =
        json!({ "arrayValue": { "values": [{ "stringValue": "stop" }] } });
    assert!(
        otlp::from_json(&d)
            .unwrap_err()
            .to_string()
            .contains("no member")
    );
}

/// Cites: TRC-28
#[test]
fn chunked_and_empty_exports_round_trip() {
    let t = trace();
    for max in [1, 2, 3, 1000] {
        let docs = otlp::to_json_chunks(&t, max).unwrap();
        assert_eq!(docs.len(), t.spans.len().div_ceil(max).max(1), "max {max}");
        for d in &docs {
            let spans: usize = d["resourceSpans"]
                .as_array()
                .unwrap()
                .iter()
                .map(|r| r["scopeSpans"][0]["spans"].as_array().unwrap().len())
                .sum();
            assert!(spans <= max);
        }
        assert_eq!(otlp::from_json_many(&docs).unwrap(), t, "max {max}");
        assert_eq!(
            otlp::to_json_chunks(&t, max).unwrap(),
            docs,
            "deterministic"
        );
    }
    let empty = otlp::to_json(&Trace::default()).unwrap();
    assert_eq!(empty, json!({ "resourceSpans": [] }));
    assert_eq!(otlp::from_json(&empty).unwrap(), Trace::default());
}

/// Cites: TRC-28
#[test]
fn export_refuses_rows_it_would_otherwise_drop() {
    let mut t = trace();
    t.spans[0].resource_id = 99;
    assert!(otlp::to_json(&t).is_err(), "a span naming no resource");
    let mut t = trace();
    let mut orphan = t.events[0].clone();
    orphan.span_id = [0xfe; 8];
    t.events.push(orphan);
    t.sort();
    assert!(otlp::to_json(&t).is_err(), "an event of no span");
}
