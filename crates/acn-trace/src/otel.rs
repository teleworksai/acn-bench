//! The bridge from the OpenTelemetry SDK to the span model (TRC-1): producers emit
//! through `opentelemetry_sdk`, and a [`Collector`] gathers what its exporters
//! receive into a [`Trace`].
//!
//! Timestamps are the run's clock (TRC-26): a producer sets every start, end and
//! event time explicitly, as `UNIX_EPOCH` plus the clock's offset, so that `sim`
//! times start at 0. Anything the SDK would drop or coerce is an error here: a
//! dropped attribute, event or link, an array value, an attribute set twice.

use std::collections::BTreeMap;
use std::sync::{Arc, Mutex};
use std::time::{SystemTime, UNIX_EPOCH};

use opentelemetry::trace::{SpanId, SpanKind, Status};
use opentelemetry::{KeyValue, Value};
use opentelemetry_sdk::Resource;
use opentelemetry_sdk::error::{OTelSdkError, OTelSdkResult};
use opentelemetry_sdk::trace::{SpanData, SpanExporter};

use crate::model::{
    AttrValue, Attrs, EventRow, LinkRow, ResourceRow, SpanRow, Trace, kind, status,
};

/// A span the model cannot hold.
#[derive(Debug, thiserror::Error)]
pub enum ConvertError {
    #[error("{0}")]
    Invalid(String),
}

fn invalid<T>(message: impl Into<String>) -> Result<T, ConvertError> {
    Err(ConvertError::Invalid(message.into()))
}

/// The resource every acn-bench producer uses (TRC-19). It is built from an empty
/// resource, so that no detector and no `OTEL_*` environment variable can add to
/// it (CON-29: no environment variable other than credentials and logging may
/// alter a run).
#[must_use]
pub fn producer_resource(
    service_name: &str,
    service_version: &str,
    engine_hash: &crate::identity::Digest,
    build_hash: &crate::identity::Digest,
) -> Resource {
    Resource::builder_empty()
        .with_attributes([
            KeyValue::new("service.name", service_name.to_owned()),
            KeyValue::new("service.version", service_version.to_owned()),
            KeyValue::new("acn.engine_hash", engine_hash.to_hex()),
            KeyValue::new("acn.build_hash", build_hash.to_hex()),
        ])
        .build()
}

#[derive(Debug, Default)]
struct Inner {
    /// Each exported span with the resource of the exporter that received it.
    spans: Vec<(SpanData, Arc<Attrs>)>,
}

/// Gathers spans from any number of exporters, one per tracer provider.
#[derive(Debug, Clone, Default)]
pub struct Collector {
    inner: Arc<Mutex<Inner>>,
}

/// An exporter feeding a [`Collector`]; give one to each tracer provider, with
/// `with_simple_exporter`.
#[derive(Debug)]
pub struct CollectorExporter {
    inner: Arc<Mutex<Inner>>,
    resource: Result<Arc<Attrs>, String>,
}

impl Collector {
    #[must_use]
    pub fn new() -> Self {
        Self::default()
    }

    /// A new exporter for one tracer provider.
    #[must_use]
    pub fn exporter(&self) -> CollectorExporter {
        CollectorExporter {
            inner: Arc::clone(&self.inner),
            resource: Ok(Arc::new(Attrs::new())),
        }
    }

    /// Everything collected so far, in the model's stored order. Resources are
    /// deduplicated and numbered in the order of their sorted attributes, so the
    /// numbering does not depend on which provider exported first.
    pub fn trace(&self) -> Result<Trace, ConvertError> {
        let inner = match self.inner.lock() {
            Ok(g) => g,
            Err(p) => p.into_inner(),
        };
        let mut distinct: Vec<Arc<Attrs>> = Vec::new();
        for (_, r) in &inner.spans {
            if !distinct.iter().any(|d| d == r) {
                distinct.push(Arc::clone(r));
            }
        }
        distinct.sort_by(|a, b| attrs_order(a, b));
        let mut trace = Trace::default();
        for (i, r) in distinct.iter().enumerate() {
            trace.resources.push(ResourceRow {
                resource_id: id32(i)?,
                attrs: (**r).clone(),
            });
        }
        for (span, r) in &inner.spans {
            let resource_id = id32(distinct.iter().position(|d| d == r).unwrap_or(0))?;
            convert(span, resource_id, &mut trace)?;
        }
        trace.sort();
        Ok(trace)
    }
}

fn id32(i: usize) -> Result<i32, ConvertError> {
    i32::try_from(i).or_else(|_| invalid("too many resources for an Int32 resource_id"))
}

/// A total order on attribute sets, entry by entry in key order: by key, then by
/// value type in union-member order, then by value (floats by `total_cmp`).
fn attrs_order(a: &Attrs, b: &Attrs) -> std::cmp::Ordering {
    use std::cmp::Ordering;
    fn rank(v: &AttrValue) -> u8 {
        match v {
            AttrValue::String(_) => 0,
            AttrValue::Int(_) => 1,
            AttrValue::Float(_) => 2,
            AttrValue::Bool(_) => 3,
            AttrValue::Bytes(_) => 4,
        }
    }
    fn value_order(x: &AttrValue, y: &AttrValue) -> Ordering {
        match (x, y) {
            (AttrValue::String(p), AttrValue::String(q)) => p.cmp(q),
            (AttrValue::Int(p), AttrValue::Int(q)) => p.cmp(q),
            (AttrValue::Float(p), AttrValue::Float(q)) => p.total_cmp(q),
            (AttrValue::Bool(p), AttrValue::Bool(q)) => p.cmp(q),
            (AttrValue::Bytes(p), AttrValue::Bytes(q)) => p.cmp(q),
            _ => rank(x).cmp(&rank(y)),
        }
    }
    for ((ka, va), (kb, vb)) in a.iter().zip(b.iter()) {
        let o = ka.cmp(kb).then_with(|| value_order(va, vb));
        if o != Ordering::Equal {
            return o;
        }
    }
    a.len().cmp(&b.len())
}

impl SpanExporter for CollectorExporter {
    async fn export(&self, batch: Vec<SpanData>) -> OTelSdkResult {
        let resource = self
            .resource
            .as_ref()
            .map_err(|e| OTelSdkError::InternalFailure(e.clone()))?;
        let mut inner = self
            .inner
            .lock()
            .map_err(|e| OTelSdkError::InternalFailure(format!("collector lock poisoned: {e}")))?;
        for span in batch {
            inner.spans.push((span, Arc::clone(resource)));
        }
        Ok(())
    }

    fn set_resource(&mut self, resource: &Resource) {
        self.resource = attrs_of(resource.iter().map(|(k, v)| (k.as_str(), v)))
            .map(Arc::new)
            .map_err(|e| e.to_string());
    }
}

fn value(key: &str, v: &Value) -> Result<AttrValue, ConvertError> {
    Ok(match v {
        Value::Bool(b) => AttrValue::Bool(*b),
        Value::I64(i) => AttrValue::Int(*i),
        Value::F64(f) => AttrValue::Float(*f),
        Value::String(s) => AttrValue::String(s.as_str().to_owned()),
        Value::Array(_) => {
            return invalid(format!(
                "attribute `{key}` is an array; the ACN profile stores scalars only (TRC-25)"
            ));
        }
        _ => {
            return invalid(format!(
                "attribute `{key}` has a value type the profile does not store"
            ));
        }
    })
}

fn attrs_of<'a>(kvs: impl Iterator<Item = (&'a str, &'a Value)>) -> Result<Attrs, ConvertError> {
    let mut out = BTreeMap::new();
    for (k, v) in kvs {
        if out.insert(k.to_owned(), value(k, v)?).is_some() {
            return invalid(format!("attribute `{k}` is set twice"));
        }
    }
    Ok(out)
}

fn kv_attrs(kvs: &[KeyValue]) -> Result<Attrs, ConvertError> {
    attrs_of(kvs.iter().map(|kv| (kv.key.as_str(), &kv.value)))
}

/// Nanoseconds since `UNIX_EPOCH`: the run clock's offset (TRC-26).
pub fn clock_ns(t: SystemTime) -> Result<i64, ConvertError> {
    let Ok(d) = t.duration_since(UNIX_EPOCH) else {
        return invalid("a timestamp before the run clock's origin");
    };
    i64::try_from(d.as_nanos()).or_else(|_| invalid("a timestamp beyond the Int64 range"))
}

fn convert(span: &SpanData, resource_id: i32, trace: &mut Trace) -> Result<(), ConvertError> {
    let name = span.name.to_string();
    if span.dropped_attributes_count > 0
        || span.events.dropped_count > 0
        || span.links.dropped_count > 0
    {
        return invalid(format!(
            "span `{name}` dropped attributes, events or links; raise the SDK span limits"
        ));
    }
    let ctx = &span.span_context;
    let trace_id = ctx.trace_id().to_bytes();
    let span_id = ctx.span_id().to_bytes();
    let parent_span_id =
        (span.parent_span_id != SpanId::INVALID).then(|| span.parent_span_id.to_bytes());
    let (status_code, status_message) = match &span.status {
        Status::Unset => (status::UNSET, None),
        Status::Ok => (status::OK, None),
        Status::Error { description } => (
            status::ERROR,
            (!description.is_empty()).then(|| description.to_string()),
        ),
    };
    let start_ns = clock_ns(span.start_time)?;
    let end_ns = clock_ns(span.end_time)?;
    if end_ns < start_ns {
        return invalid(format!("span `{name}` ends before it starts"));
    }
    trace.spans.push(SpanRow {
        trace_id,
        span_id,
        parent_span_id,
        name: name.clone(),
        kind: match span.span_kind {
            SpanKind::Internal => kind::INTERNAL,
            SpanKind::Server => kind::SERVER,
            SpanKind::Client => kind::CLIENT,
            SpanKind::Producer => kind::PRODUCER,
            SpanKind::Consumer => kind::CONSUMER,
        },
        start_ns,
        end_ns,
        status_code,
        status_message,
        resource_id,
        attrs: kv_attrs(&span.attributes)?,
    });
    for (seq, e) in span.events.iter().enumerate() {
        if e.dropped_attributes_count > 0 {
            return invalid(format!("an event of span `{name}` dropped attributes"));
        }
        trace.events.push(EventRow {
            trace_id,
            span_id,
            seq: u32::try_from(seq).or_else(|_| invalid("too many events"))?,
            time_ns: clock_ns(e.timestamp)?,
            name: e.name.to_string(),
            attrs: kv_attrs(&e.attributes)?,
        });
    }
    for (seq, l) in span.links.iter().enumerate() {
        if l.dropped_attributes_count > 0 {
            return invalid(format!("a link of span `{name}` dropped attributes"));
        }
        trace.links.push(LinkRow {
            trace_id,
            span_id,
            seq: u32::try_from(seq).or_else(|_| invalid("too many links"))?,
            linked_trace_id: l.span_context.trace_id().to_bytes(),
            linked_span_id: l.span_context.span_id().to_bytes(),
            attrs: kv_attrs(&l.attributes)?,
        });
    }
    Ok(())
}
