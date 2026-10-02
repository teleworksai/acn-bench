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
