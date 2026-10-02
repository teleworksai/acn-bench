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

impl Trace {
    /// Put every table in its stored order: spans by `(start_ns, trace_id, span_id)`
    /// (TRC-25), events by `(time_ns, trace_id, span_id, seq)`, links by
    /// `(trace_id, span_id, seq)`, resources by id.
    pub fn sort(&mut self) {
        self.spans.sort_by(|a, b| {
            (a.start_ns, a.trace_id, a.span_id).cmp(&(b.start_ns, b.trace_id, b.span_id))
        });
        self.events.sort_by(|a, b| {
            (a.time_ns, a.trace_id, a.span_id, a.seq)
                .cmp(&(b.time_ns, b.trace_id, b.span_id, b.seq))
        });
        self.links
            .sort_by(|a, b| (a.trace_id, a.span_id, a.seq).cmp(&(b.trace_id, b.span_id, b.seq)));
        self.resources.sort_by_key(|r| r.resource_id);
    }
}
