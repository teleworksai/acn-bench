//! OTLP/JSON (TRC-28): a trace as the JSON encoding of an OTLP
//! `ExportTraceServiceRequest`, the form an OTLP/HTTP collector accepts at
//! `/v1/traces` with `Content-Type: application/json`, and back.
//!
//! The encoding follows the OTLP JSON rules: trace and span ids as lowercase hex,
//! 64-bit integers as decimal strings, bytes as base64, enums as integers. Export
//! and import are inverse on everything the model holds: a span's events and links
//! keep their order (`seq`), and resources are numbered on import by their sorted
//! attributes, the rule the collector uses, so a bundle exported and re-imported
//! yields the same tables and the same views. Import refuses what the model cannot
//! hold faithfully: array and map values, dropped attributes, events or links, a
//! non-finite double, an attribute set twice, a malformed id or time.

use base64::Engine as _;
use base64::engine::general_purpose::STANDARD;
use serde_json::{Map, Value, json};

use crate::model::{AttrValue, Attrs, EventRow, LinkRow, ResourceRow, SpanRow, Trace};

/// A document that is not OTLP/JSON the model can hold.
#[derive(Debug, thiserror::Error)]
pub enum OtlpError {
    #[error("{0}")]
    Invalid(String),
}

type Result<T> = std::result::Result<T, OtlpError>;

fn invalid<T>(message: impl Into<String>) -> Result<T> {
    Err(OtlpError::Invalid(message.into()))
}

/// The instrumentation scope exported spans are grouped under.
pub const SCOPE: &str = "acn-bench";

fn hex(bytes: &[u8]) -> String {
    bytes.iter().map(|b| format!("{b:02x}")).collect()
}

fn value_json(v: &AttrValue) -> Value {
    match v {
        AttrValue::String(s) => json!({ "stringValue": s }),
        AttrValue::Int(i) => json!({ "intValue": i.to_string() }),
        AttrValue::Float(f) => json!({ "doubleValue": f }),
        AttrValue::Bool(b) => json!({ "boolValue": b }),
        AttrValue::Bytes(b) => json!({ "bytesValue": STANDARD.encode(b) }),
    }
}

fn attrs_json(attrs: &Attrs) -> Value {
    Value::Array(
        attrs
            .iter()
            .map(|(k, v)| json!({ "key": k, "value": value_json(v) }))
            .collect(),
    )
}

/// Encode a trace (in stored order) as an OTLP/JSON document: one `resourceSpans`
/// entry per resource, in resource-id order, holding that resource's spans in
/// stored order.
pub fn to_json(trace: &Trace) -> Result<Value> {
    if !trace.is_sorted() {
        return invalid("the trace is not in stored order (TRC-25)");
    }
    let mut resource_spans = Vec::new();
    for r in &trace.resources {
        let mut spans = Vec::new();
        for s in trace
            .spans
            .iter()
            .filter(|s| s.resource_id == r.resource_id)
        {
            let key = (s.trace_id, s.span_id);
            let mut events: Vec<&EventRow> = trace
                .events
                .iter()
                .filter(|e| (e.trace_id, e.span_id) == key)
                .collect();
            events.sort_by_key(|e| e.seq);
            let mut links: Vec<&LinkRow> = trace
                .links
                .iter()
                .filter(|l| (l.trace_id, l.span_id) == key)
                .collect();
            links.sort_by_key(|l| l.seq);
            let mut status = Map::new();
            status.insert("code".into(), json!(s.status_code));
            if let Some(m) = &s.status_message {
                status.insert("message".into(), json!(m));
            }
            let mut span = Map::new();
            span.insert("traceId".into(), json!(hex(&s.trace_id)));
            span.insert("spanId".into(), json!(hex(&s.span_id)));
            if let Some(p) = s.parent_span_id {
                span.insert("parentSpanId".into(), json!(hex(&p)));
            }
            span.insert("name".into(), json!(s.name));
            span.insert("kind".into(), json!(s.kind));
            span.insert("startTimeUnixNano".into(), json!(s.start_ns.to_string()));
            span.insert("endTimeUnixNano".into(), json!(s.end_ns.to_string()));
            span.insert("attributes".into(), attrs_json(&s.attrs));
            span.insert(
                "events".into(),
                Value::Array(
                    events
                        .iter()
                        .map(|e| {
                            json!({
                                "timeUnixNano": e.time_ns.to_string(),
                                "name": e.name,
                                "attributes": attrs_json(&e.attrs),
                            })
                        })
                        .collect(),
                ),
            );
            span.insert(
                "links".into(),
                Value::Array(
                    links
                        .iter()
                        .map(|l| {
                            json!({
                                "traceId": hex(&l.linked_trace_id),
                                "spanId": hex(&l.linked_span_id),
                                "attributes": attrs_json(&l.attrs),
                            })
                        })
                        .collect(),
                ),
            );
            span.insert("status".into(), Value::Object(status));
            spans.push(Value::Object(span));
        }
        resource_spans.push(json!({
            "resource": { "attributes": attrs_json(&r.attrs) },
            "scopeSpans": [{ "scope": { "name": SCOPE }, "spans": spans }],
        }));
    }
    Ok(json!({ "resourceSpans": resource_spans }))
}

// ---- import ----

fn field<'a>(o: &'a Value, key: &str, at: &str) -> Result<&'a Value> {
    o.get(key)
        .map_or_else(|| invalid(format!("{at}: `{key}` is missing")), Ok)
}

fn array<'a>(o: &'a Value, key: &str) -> Result<&'a [Value]> {
    match o.get(key) {
        None | Some(Value::Null) => Ok(&[]),
        Some(Value::Array(a)) => Ok(a),
        Some(_) => invalid(format!("`{key}` is not an array")),
    }
}

fn string<'a>(v: &'a Value, at: &str) -> Result<&'a str> {
    v.as_str()
        .map_or_else(|| invalid(format!("{at} is not a string")), Ok)
}

/// An int64 written as a decimal string or, leniently, as a JSON integer.
fn int64(v: &Value, at: &str) -> Result<i64> {
    match v {
        Value::String(s) => s
            .parse()
            .or_else(|_| invalid(format!("{at}: `{s}` is not an int64"))),
        Value::Number(n) => n
            .as_i64()
            .map_or_else(|| invalid(format!("{at}: {n} is not an int64")), Ok),
        _ => invalid(format!("{at} is not an int64")),
    }
}

fn small_int<T: TryFrom<i64>>(v: &Value, at: &str) -> Result<T> {
    let i = int64(v, at)?;
    T::try_from(i).or_else(|_| invalid(format!("{at}: {i} is out of range")))
}

fn id<const N: usize>(v: &Value, at: &str) -> Result<[u8; N]> {
    let s = string(v, at)?;
    let ok = s.len() == 2 * N && s.bytes().all(|b| b.is_ascii_hexdigit());
    if !ok {
        return invalid(format!("{at}: `{s}` is not {N} bytes of hex"));
    }
    let mut out = [0u8; N];
    for (i, b) in out.iter_mut().enumerate() {
        *b = u8::from_str_radix(&s[2 * i..2 * i + 2], 16)
            .or_else(|_| invalid(format!("{at}: `{s}` is not hex")))?;
    }
    Ok(out)
}

fn no_drops(o: &Value, at: &str) -> Result<()> {
    for key in [
        "droppedAttributesCount",
        "droppedEventsCount",
        "droppedLinksCount",
    ] {
        if let Some(v) = o.get(key)
            && int64(v, at)? != 0
        {
            return invalid(format!("{at}: the producer dropped data (`{key}`)"));
        }
    }
    Ok(())
}

fn attr_value(v: &Value, at: &str) -> Result<AttrValue> {
    let Value::Object(m) = v else {
        return invalid(format!("{at}: a value is not an object"));
    };
    let mut kinds = m.iter();
    let (Some((kind, inner)), None) = (kinds.next(), kinds.next()) else {
        return invalid(format!("{at}: a value must set exactly one member"));
    };
    Ok(match kind.as_str() {
        "stringValue" => AttrValue::String(string(inner, at)?.to_owned()),
        "intValue" => AttrValue::Int(int64(inner, at)?),
        "doubleValue" => {
            let f = match inner {
                Value::Number(n) => n.as_f64(),
                _ => None,
            };
            match f {
                Some(f) if f.is_finite() => AttrValue::Float(if f == 0.0 { 0.0 } else { f }),
                _ => return invalid(format!("{at}: a double must be a finite number")),
            }
        }
        "boolValue" => match inner {
            Value::Bool(b) => AttrValue::Bool(*b),
            _ => return invalid(format!("{at}: boolValue is not a bool")),
        },
        "bytesValue" => AttrValue::Bytes(
            STANDARD
                .decode(string(inner, at)?)
                .or_else(|e| invalid(format!("{at}: bytesValue is not base64 ({e})")))?,
        ),
        other => {
            return invalid(format!(
                "{at}: `{other}` values have no member in the ACN profile (TRC-25)"
            ));
        }
    })
}

fn attrs(o: &Value, at: &str) -> Result<Attrs> {
    let mut out = Attrs::new();
    for kv in array(o, "attributes")? {
        let key = string(field(kv, "key", at)?, at)?;
        let value = attr_value(field(kv, "value", at)?, &format!("{at}, `{key}`"))?;
        if out.insert(key.to_owned(), value).is_some() {
            return invalid(format!("{at}: attribute `{key}` is set twice"));
        }
    }
    Ok(out)
}

/// Decode an OTLP/JSON `ExportTraceServiceRequest` into a trace in stored order.
pub fn from_json(doc: &Value) -> Result<Trace> {
    let mut resources: Vec<Attrs> = Vec::new();
    let mut spans: Vec<(usize, SpanRow)> = Vec::new();
    let mut trace = Trace::default();
    for (ri, rs) in array(doc, "resourceSpans")?.iter().enumerate() {
        let at = format!("resourceSpans[{ri}]");
        let resource = rs.get("resource").cloned().unwrap_or(Value::Null);
        no_drops(&resource, &at)?;
        let r_attrs = attrs(&resource, &at)?;
        let r_index = match resources
            .iter()
            .position(|r| crate::otel::attrs_order(r, &r_attrs).is_eq())
        {
            Some(i) => i,
            None => {
                resources.push(r_attrs);
                resources.len() - 1
            }
        };
        for (si, ss) in array(rs, "scopeSpans")?.iter().enumerate() {
            for (pi, sp) in array(ss, "spans")?.iter().enumerate() {
                let at = format!("resourceSpans[{ri}].scopeSpans[{si}].spans[{pi}]");
                no_drops(sp, &at)?;
                let trace_id = id::<16>(field(sp, "traceId", &at)?, &at)?;
                let span_id = id::<8>(field(sp, "spanId", &at)?, &at)?;
                let parent_span_id = match sp.get("parentSpanId") {
                    None | Some(Value::Null) => None,
                    Some(Value::String(s)) if s.is_empty() => None,
                    Some(v) => Some(id::<8>(v, &at)?),
                };
                let status = sp.get("status").cloned().unwrap_or(Value::Null);
                let status_code = match status.get("code") {
                    None => 0,
                    Some(v) => small_int::<i8>(v, &at)?,
                };
                let status_message = match status.get("message") {
                    None | Some(Value::Null) => None,
                    Some(Value::String(s)) if s.is_empty() => None,
                    Some(v) => Some(string(v, &at)?.to_owned()),
                };
                for (ei, e) in array(sp, "events")?.iter().enumerate() {
                    let eat = format!("{at}.events[{ei}]");
                    no_drops(e, &eat)?;
                    trace.events.push(EventRow {
                        trace_id,
                        span_id,
                        seq: u32::try_from(ei).or_else(|_| invalid("too many events"))?,
                        time_ns: int64(field(e, "timeUnixNano", &eat)?, &eat)?,
                        name: string(field(e, "name", &eat)?, &eat)?.to_owned(),
                        attrs: attrs(e, &eat)?,
                    });
                }
                for (li, l) in array(sp, "links")?.iter().enumerate() {
                    let lat = format!("{at}.links[{li}]");
                    no_drops(l, &lat)?;
                    trace.links.push(LinkRow {
                        trace_id,
                        span_id,
                        seq: u32::try_from(li).or_else(|_| invalid("too many links"))?,
                        linked_trace_id: id::<16>(field(l, "traceId", &lat)?, &lat)?,
                        linked_span_id: id::<8>(field(l, "spanId", &lat)?, &lat)?,
                        attrs: attrs(l, &lat)?,
                    });
                }
                spans.push((
                    r_index,
                    SpanRow {
                        trace_id,
                        span_id,
                        parent_span_id,
                        name: string(field(sp, "name", &at)?, &at)?.to_owned(),
                        kind: match sp.get("kind") {
                            None => 0,
                            Some(v) => small_int::<i8>(v, &at)?,
                        },
                        start_ns: int64(field(sp, "startTimeUnixNano", &at)?, &at)?,
                        end_ns: int64(field(sp, "endTimeUnixNano", &at)?, &at)?,
                        status_code,
                        status_message,
                        resource_id: 0,
                        attrs: attrs(sp, &at)?,
                    },
                ));
            }
        }
    }
    // Number the resources by their sorted attributes, as the collector does.
    let mut order: Vec<usize> = (0..resources.len()).collect();
    order.sort_by(|&a, &b| crate::otel::attrs_order(&resources[a], &resources[b]));
    let id_of = |index: usize| -> Result<i32> {
        let Some(pos) = order.iter().position(|&o| o == index) else {
            return invalid("internal: a resource was not numbered");
        };
        i32::try_from(pos).or_else(|_| invalid("too many resources"))
    };
    for (pos, &index) in order.iter().enumerate() {
        trace.resources.push(ResourceRow {
            resource_id: i32::try_from(pos).or_else(|_| invalid("too many resources"))?,
            attrs: resources[index].clone(),
        });
    }
    for (index, mut span) in spans {
        span.resource_id = id_of(index)?;
        trace.spans.push(span);
    }
    trace.sort();
    Ok(trace)
}
