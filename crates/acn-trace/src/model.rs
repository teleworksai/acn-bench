//! The span model: what a bundle stores, row by row, before it becomes Parquet
//! (SPEC 010 §6). It mirrors OTLP — spans, span events, span links and resources —
//! with the attribute union restricted to the five members TRC-25 names.

use std::collections::BTreeMap;

/// An attribute value: the OTLP union restricted to `string`, `int`, `float`,
/// `bool` and `bytes` (TRC-25). OTLP arrays and maps have no member here; a producer
/// that sets one gets an error, never a silent conversion.
#[derive(Debug, Clone, PartialEq)]
pub enum AttrValue {
    String(String),
    Int(i64),
    Float(f64),
    Bool(bool),
    Bytes(Vec<u8>),
}

impl AttrValue {
    /// A float attribute. Negative zero is stored as zero, as in the text form of
    /// CON-27(c); the caller has refused NaN and infinities.
    #[must_use]
    pub fn float(f: f64) -> Self {
        Self::Float(if f == 0.0 { 0.0 } else { f })
    }

    /// The schema type of this value.
    #[must_use]
    pub fn ty(&self) -> crate::schema::ValueType {
        use crate::schema::ValueType as T;
        match self {
            Self::String(_) => T::String,
            Self::Int(_) => T::Int,
            Self::Float(_) => T::Float,
            Self::Bool(_) => T::Bool,
            Self::Bytes(_) => T::Bytes,
        }
    }
}

/// Attributes, keyed in bytewise order.
pub type Attrs = BTreeMap<String, AttrValue>;

/// OTLP span kind codes, as stored in the `kind` column.
pub mod kind {
    pub const UNSPECIFIED: i8 = 0;
    pub const INTERNAL: i8 = 1;
    pub const SERVER: i8 = 2;
    pub const CLIENT: i8 = 3;
    pub const PRODUCER: i8 = 4;
    pub const CONSUMER: i8 = 5;
}

/// OTLP status codes, as stored in the `status_code` column.
pub mod status {
    pub const UNSET: i8 = 0;
    pub const OK: i8 = 1;
    pub const ERROR: i8 = 2;
}

/// One row of `spans.parquet`.
#[derive(Debug, Clone, PartialEq)]
pub struct SpanRow {
    pub trace_id: [u8; 16],
    pub span_id: [u8; 8],
    pub parent_span_id: Option<[u8; 8]>,
    pub name: String,
    pub kind: i8,
    /// Nanoseconds on the run's clock (TRC-26).
    pub start_ns: i64,
    pub end_ns: i64,
    pub status_code: i8,
    pub status_message: Option<String>,
    pub resource_id: i32,
    pub attrs: Attrs,
}

/// One row of `events.parquet`. `seq` is the event's position among its span's
/// events, which orders events that share a timestamp.
#[derive(Debug, Clone, PartialEq)]
pub struct EventRow {
    pub trace_id: [u8; 16],
    pub span_id: [u8; 8],
    pub seq: u32,
    pub time_ns: i64,
    pub name: String,
    pub attrs: Attrs,
}

/// One row of `links.parquet`.
#[derive(Debug, Clone, PartialEq)]
pub struct LinkRow {
    pub trace_id: [u8; 16],
    pub span_id: [u8; 8],
    pub seq: u32,
    pub linked_trace_id: [u8; 16],
    pub linked_span_id: [u8; 8],
    pub attrs: Attrs,
}

/// One row of `resources.parquet`.
#[derive(Debug, Clone, PartialEq)]
pub struct ResourceRow {
    pub resource_id: i32,
    pub attrs: Attrs,
}

/// Everything one run recorded.
#[derive(Debug, Clone, Default, PartialEq)]
pub struct Trace {
    pub spans: Vec<SpanRow>,
    pub events: Vec<EventRow>,
    pub links: Vec<LinkRow>,
    pub resources: Vec<ResourceRow>,
}

impl SpanRow {
    /// The stored-order key (TRC-25).
    #[must_use]
    pub fn key(&self) -> (i64, [u8; 16], [u8; 8]) {
        (self.start_ns, self.trace_id, self.span_id)
    }
}

impl EventRow {
    /// The stored-order key.
    #[must_use]
    pub fn key(&self) -> (i64, [u8; 16], [u8; 8], u32) {
        (self.time_ns, self.trace_id, self.span_id, self.seq)
    }
}

impl LinkRow {
    /// The stored-order key.
    #[must_use]
    pub fn key(&self) -> ([u8; 16], [u8; 8], u32) {
        (self.trace_id, self.span_id, self.seq)
    }
}

impl Trace {
    /// Put every table in its stored order: spans by `(start_ns, trace_id, span_id)`
    /// (TRC-25), events by `(time_ns, trace_id, span_id, seq)`, links by
    /// `(trace_id, span_id, seq)`, resources by id.
    pub fn sort(&mut self) {
        self.spans.sort_by_key(SpanRow::key);
        self.events.sort_by_key(EventRow::key);
        self.links.sort_by_key(LinkRow::key);
        self.resources.sort_by_key(|r| r.resource_id);
    }

    /// Whether every table is in stored order, each key once. Compares keys only,
    /// so a float attribute value cannot make an ordered trace look unordered.
    #[must_use]
    pub fn is_sorted(&self) -> bool {
        self.spans.windows(2).all(|w| w[0].key() < w[1].key())
            && self.events.windows(2).all(|w| w[0].key() < w[1].key())
            && self.links.windows(2).all(|w| w[0].key() < w[1].key())
            && self
                .resources
                .windows(2)
                .all(|w| w[0].resource_id < w[1].resource_id)
    }
}

/// A total order on attribute sets, entry by entry in key order: by key, then by
/// value type in union-member order, then by value (floats by `total_cmp`).
pub(crate) fn attrs_order(a: &Attrs, b: &Attrs) -> std::cmp::Ordering {
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

impl Trace {
    /// Deduplicate the resources and number them in the order of their sorted
    /// attributes, remapping every span: the one numbering rule the collector,
    /// OTLP import and merging share, so it never depends on input order. A span
    /// naming a resource the trace does not hold is an error.
    pub fn renumber_resources(&mut self) -> Result<(), String> {
        let mut distinct: Vec<Attrs> = Vec::new();
        for r in &self.resources {
            if !distinct.iter().any(|d| attrs_order(d, &r.attrs).is_eq()) {
                distinct.push(r.attrs.clone());
            }
        }
        distinct.sort_by(attrs_order);
        let new_id = |attrs: &Attrs| -> Result<i32, String> {
            let i = distinct
                .iter()
                .position(|d| attrs_order(d, attrs).is_eq())
                .ok_or("internal: a resource was not numbered")?;
            i32::try_from(i).map_err(|_| "too many resources for an Int32 resource_id".to_owned())
        };
        let mut remap = std::collections::BTreeMap::new();
        for r in &self.resources {
            remap.insert(r.resource_id, new_id(&r.attrs)?);
        }
        for s in &mut self.spans {
            s.resource_id = *remap.get(&s.resource_id).ok_or_else(|| {
                format!(
                    "span `{}` names resource {} that the trace does not hold",
                    s.name, s.resource_id
                )
            })?;
        }
        self.resources = distinct
            .into_iter()
            .enumerate()
            .map(|(i, attrs)| {
                Ok(ResourceRow {
                    resource_id: i32::try_from(i).map_err(|_| "too many resources".to_owned())?,
                    attrs,
                })
            })
            .collect::<Result<_, String>>()?;
        self.sort();
        Ok(())
    }

    /// Two traces as one: `other`'s resources are renumbered together with this
    /// trace's (TRC-28: a node's own OTLP dump merged with the run it answered).
    pub fn merge(mut self, mut other: Trace) -> Result<Trace, String> {
        let offset = self
            .resources
            .iter()
            .map(|r| r.resource_id)
            .max()
            .map_or(Some(0), |m| m.checked_add(1))
            .ok_or("too many resources")?;
        for r in &mut other.resources {
            r.resource_id = r
                .resource_id
                .checked_add(offset)
                .ok_or("too many resources")?;
        }
        for s in &mut other.spans {
            s.resource_id = s
                .resource_id
                .checked_add(offset)
                .ok_or("too many resources")?;
        }
        self.spans.append(&mut other.spans);
        self.events.append(&mut other.events);
        self.links.append(&mut other.links);
        self.resources.append(&mut other.resources);
        self.renumber_resources()?;
        Ok(self)
    }
}
