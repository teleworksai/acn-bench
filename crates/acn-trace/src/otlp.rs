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

fn span_json(s: &SpanRow, events: &[&EventRow], links: &[&LinkRow]) -> Value {
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
    Value::Object(span)
}

/// Encode a trace (in stored order) as OTLP/JSON documents of at most
/// `max_spans` spans each (`0`: one document). Each document holds one
/// `resourceSpans` entry per resource it has spans of, in resource-id order, and
/// those spans in stored order with all their events and links. The split is a
/// function of the trace alone, so the documents are deterministic. Every span
/// must name a resource the trace holds, and every event and link a span.
pub fn to_json_chunks(trace: &Trace, max_spans: usize) -> Result<Vec<Value>> {
    use std::collections::BTreeMap;
    if !trace.is_sorted() {
        return invalid("the trace is not in stored order (TRC-25)");
    }
    type Key = ([u8; 16], [u8; 8]);
    let mut events: BTreeMap<Key, Vec<&EventRow>> = BTreeMap::new();
    for e in &trace.events {
        events.entry((e.trace_id, e.span_id)).or_default().push(e);
    }
    let mut links: BTreeMap<Key, Vec<&LinkRow>> = BTreeMap::new();
    for l in &trace.links {
        links.entry((l.trace_id, l.span_id)).or_default().push(l);
    }
    let resources: BTreeMap<i32, &ResourceRow> =
        trace.resources.iter().map(|r| (r.resource_id, r)).collect();
    // Spans in (resource, stored) order: the order of the documents.
    let mut ordered: Vec<&SpanRow> = Vec::with_capacity(trace.spans.len());
    let mut held = std::collections::BTreeSet::new();
    for s in &trace.spans {
        if !resources.contains_key(&s.resource_id) {
            return invalid(format!(
                "span `{}` names resource {} that the trace does not hold",
                s.name, s.resource_id
            ));
        }
        held.insert((s.trace_id, s.span_id));
        ordered.push(s);
    }
    ordered.sort_by_key(|s| s.resource_id);
    if events.keys().chain(links.keys()).any(|k| !held.contains(k)) {
        return invalid("an event or link belongs to no span of the trace");
    }
    let size = if max_spans == 0 {
        ordered.len().max(1)
    } else {
        max_spans
    };
    let mut docs = Vec::new();
    for chunk in ordered.chunks(size) {
        let mut groups: Vec<(i32, Vec<Value>)> = Vec::new();
        for s in chunk {
            let key = (s.trace_id, s.span_id);
            let mut ev = events.get(&key).cloned().unwrap_or_default();
            ev.sort_by_key(|e| e.seq);
            let mut ln = links.get(&key).cloned().unwrap_or_default();
            ln.sort_by_key(|l| l.seq);
            let json = span_json(s, &ev, &ln);
            match groups.last_mut() {
                Some((rid, spans)) if *rid == s.resource_id => spans.push(json),
                _ => groups.push((s.resource_id, vec![json])),
            }
        }
        let resource_spans: Vec<Value> = groups
            .into_iter()
            .filter_map(|(rid, spans)| {
                resources.get(&rid).map(|r| {
                    json!({
                        "resource": { "attributes": attrs_json(&r.attrs) },
                        "scopeSpans": [{ "scope": { "name": SCOPE }, "spans": spans }],
                    })
                })
            })
            .collect();
        docs.push(json!({ "resourceSpans": resource_spans }));
    }
    if docs.is_empty() {
        docs.push(json!({ "resourceSpans": [] }));
    }
    Ok(docs)
}

/// The whole trace as one OTLP/JSON document (see [`to_json_chunks`]).
pub fn to_json(trace: &Trace) -> Result<Value> {
    let mut docs = to_json_chunks(trace, 0)?;
    docs.pop()
        .map_or_else(|| invalid("internal: no document"), Ok)
}

/// Decode several documents (the chunks of one export, or a run and a node's own
/// dump) into one trace.
pub fn from_json_many(docs: &[Value]) -> Result<Trace> {
    let mut trace = Trace::default();
    for d in docs {
        trace = trace.merge(from_json(d)?).map_err(OtlpError::Invalid)?;
    }
    Ok(trace)
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

/// An int64 field that proto3 JSON omits when it is 0.
fn int64_or_zero(o: &Value, key: &str, at: &str) -> Result<i64> {
    match o.get(key) {
        None | Some(Value::Null) => Ok(0),
        Some(v) => int64(v, &format!("{at}, `{key}`")),
    }
}

/// A string field that proto3 JSON omits when it is empty.
fn string_or_empty(o: &Value, key: &str, at: &str) -> Result<String> {
    match o.get(key) {
        None | Some(Value::Null) => Ok(String::new()),
        Some(v) => Ok(string(v, &format!("{at}, `{key}`"))?.to_owned()),
    }
}

/// An OTLP enum, by number or by its proto name, omitted when 0, within `0..=max`.
fn enum_field(o: &Value, key: &str, names: &[&str], at: &str) -> Result<i8> {
    let n = match o.get(key) {
        None | Some(Value::Null) => 0,
        Some(Value::String(name)) if names.contains(&name.as_str()) => names
            .iter()
            .position(|n| n == name)
            .and_then(|p| i64::try_from(p).ok())
            .unwrap_or(0),
        Some(v) => int64(v, &format!("{at}, `{key}`"))?,
    };
    if usize::try_from(n).map_or(true, |u| u >= names.len()) {
        return invalid(format!(
            "{at}: `{key}` {n} is out of range 0..{}",
            names.len() - 1
        ));
    }
    i8::try_from(n).or_else(|_| invalid(format!("{at}: `{key}` is out of range")))
}

const KINDS: &[&str] = &[
    "SPAN_KIND_UNSPECIFIED",
    "SPAN_KIND_INTERNAL",
    "SPAN_KIND_SERVER",
    "SPAN_KIND_CLIENT",
    "SPAN_KIND_PRODUCER",
    "SPAN_KIND_CONSUMER",
];
const STATUS_CODES: &[&str] = &["STATUS_CODE_UNSET", "STATUS_CODE_OK", "STATUS_CODE_ERROR"];

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
            // proto3 JSON allows a double as a number or as a numeric string.
            let f = match inner {
                Value::Number(n) => n.as_f64(),
                Value::String(s) => s.parse::<f64>().ok(),
                _ => None,
            };
            match f {
                Some(f) if f.is_finite() => AttrValue::float(f),
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
                let status_code = enum_field(&status, "code", STATUS_CODES, &at)?;
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
                        time_ns: int64_or_zero(e, "timeUnixNano", &eat)?,
                        name: string_or_empty(e, "name", &eat)?,
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
                        name: string_or_empty(sp, "name", &at)?,
                        kind: enum_field(sp, "kind", KINDS, &at)?,
                        start_ns: int64_or_zero(sp, "startTimeUnixNano", &at)?,
                        end_ns: int64_or_zero(sp, "endTimeUnixNano", &at)?,
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
    for (index, attrs) in resources.into_iter().enumerate() {
        trace.resources.push(ResourceRow {
            resource_id: i32::try_from(index).or_else(|_| invalid("too many resources"))?,
            attrs,
        });
    }
    for (index, mut span) in spans {
        span.resource_id = i32::try_from(index).or_else(|_| invalid("too many resources"))?;
        trace.spans.push(span);
    }
    trace.renumber_resources().map_err(OtlpError::Invalid)?;
    trace.sort();
    Ok(trace)
}
