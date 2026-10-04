//! The OTLP-shaped Parquet writer (TRC-25). One file per table; the writer settings
//! are the constants of `schema::parquet`, each set explicitly. Rows are written in
//! the order of [`Trace::sort`]. Every `acn.*` attribute must be listed in the
//! inventory, and a promoted one must have its declared type, so a bundle can never
//! hold a name or a type the schema does not define (TRC-20).

use std::fs::{File, OpenOptions};
use std::path::Path;
use std::sync::Arc;

use arrow_array::builder::ArrayBuilder as _;
use arrow_array::builder::{
    BinaryBuilder, BooleanBuilder, FixedSizeBinaryBuilder, Float64Builder, Int8Builder,
    Int32Builder, Int64Builder, StringBuilder, StringDictionaryBuilder,
};
use arrow_array::types::Int32Type;
use arrow_array::{ArrayRef, MapArray, RecordBatch, StructArray};
use arrow_buffer::OffsetBuffer;
use arrow_schema::{DataType, Field, FieldRef, Fields, Schema};
use parquet::arrow::ArrowWriter;
use parquet::basic::{Compression, ZstdLevel};
use parquet::file::properties::{EnabledStatistics, WriterProperties, WriterVersion};

use crate::model::{AttrValue, Attrs, Trace};
use crate::schema::{self, Inventory, ValueType};

/// A table that cannot be written.
#[derive(Debug, thiserror::Error)]
pub enum WriteError {
    #[error("{0}")]
    Invalid(String),
    #[error("arrow: {0}")]
    Arrow(#[from] arrow_schema::ArrowError),
    #[error("parquet: {0}")]
    Parquet(#[from] parquet::errors::ParquetError),
    #[error("io error at {path}: {source}")]
    Io {
        path: String,
        source: std::io::Error,
    },
}

type Result<T> = std::result::Result<T, WriteError>;

fn invalid<T>(message: impl Into<String>) -> Result<T> {
    Err(WriteError::Invalid(message.into()))
}

/// The table files of a bundle, relative to its directory (TRC-22).
pub const SPANS: &str = "spans.parquet";
pub const EVENTS: &str = "events.parquet";
pub const LINKS: &str = "links.parquet";
pub const RESOURCES: &str = "resources.parquet";

/// The writer properties of TRC-25.
pub fn writer_properties() -> Result<WriterProperties> {
    use schema::parquet as s;
    Ok(WriterProperties::builder()
        .set_writer_version(if s::WRITER_VERSION_1_0 {
            WriterVersion::PARQUET_1_0
        } else {
            WriterVersion::PARQUET_2_0
        })
        .set_compression(Compression::ZSTD(ZstdLevel::try_new(s::ZSTD_LEVEL)?))
        .set_max_row_group_row_count(Some(s::ROW_GROUP_ROWS))
        .set_max_row_group_bytes(None)
        .set_statistics_enabled(if s::PAGE_STATISTICS {
            EnabledStatistics::Page
        } else {
            EnabledStatistics::None
        })
        .set_dictionary_enabled(s::DICTIONARY)
        .set_data_page_size_limit(s::DATA_PAGE_BYTES)
        .set_dictionary_page_size_limit(s::DICTIONARY_PAGE_BYTES)
        .set_created_by(s::CREATED_BY.to_owned())
        .set_key_value_metadata(None)
        .build())
}

/// The value type of the `attrs` map: a struct of the union members, exactly one
/// of which is set (ADR-13).
fn attr_value_fields() -> Fields {
    let [s, i, f, b, y] = schema::parquet::ATTR_MEMBERS else {
        return Fields::empty();
    };
    Fields::from(vec![
        Field::new(*s, DataType::Utf8, true),
        Field::new(*i, DataType::Int64, true),
        Field::new(*f, DataType::Float64, true),
        Field::new(*b, DataType::Boolean, true),
        Field::new(*y, DataType::Binary, true),
    ])
}

fn attrs_entries_field() -> FieldRef {
    Arc::new(Field::new(
        "entries",
        DataType::Struct(Fields::from(vec![
            Field::new("key", DataType::Utf8, false),
            Field::new("value", DataType::Struct(attr_value_fields()), false),
        ])),
        false,
    ))
}

fn attrs_field() -> Field {
    Field::new("attrs", DataType::Map(attrs_entries_field(), true), false)
}

fn arrow_type(ty: ValueType) -> DataType {
    match ty {
        ValueType::String => DataType::Utf8,
        ValueType::Int => DataType::Int64,
        ValueType::Float => DataType::Float64,
        ValueType::Bool => DataType::Boolean,
        ValueType::Bytes => DataType::Binary,
    }
}

fn dict_utf8() -> DataType {
    DataType::Dictionary(Box::new(DataType::Int32), Box::new(DataType::Utf8))
}

/// The schema of `spans.parquet`: the OTLP columns, then one typed, nullable column
/// per promoted attribute in inventory order (TRC-25).
#[must_use]
pub fn spans_schema(inv: &Inventory) -> Schema {
    let mut fields = vec![
        Field::new("trace_id", DataType::FixedSizeBinary(16), false),
        Field::new("span_id", DataType::FixedSizeBinary(8), false),
        Field::new("parent_span_id", DataType::FixedSizeBinary(8), true),
        Field::new("name", dict_utf8(), false),
        Field::new("kind", DataType::Int8, false),
        Field::new("start_ns", DataType::Int64, false),
        Field::new("end_ns", DataType::Int64, false),
        Field::new("status_code", DataType::Int8, false),
        Field::new("status_message", DataType::Utf8, true),
        Field::new("resource_id", DataType::Int32, false),
        attrs_field(),
    ];
    for a in inv.promoted() {
        fields.push(Field::new(a.column_name(), arrow_type(a.ty), true));
    }
    Schema::new(fields)
}

/// The schema of `events.parquet`.
#[must_use]
pub fn events_schema() -> Schema {
    Schema::new(vec![
        Field::new("trace_id", DataType::FixedSizeBinary(16), false),
        Field::new("span_id", DataType::FixedSizeBinary(8), false),
        Field::new("seq", DataType::Int64, false),
        Field::new("time_ns", DataType::Int64, false),
        Field::new("name", dict_utf8(), false),
        attrs_field(),
    ])
}

/// The schema of `links.parquet`.
#[must_use]
pub fn links_schema() -> Schema {
    Schema::new(vec![
        Field::new("trace_id", DataType::FixedSizeBinary(16), false),
        Field::new("span_id", DataType::FixedSizeBinary(8), false),
        Field::new("seq", DataType::Int64, false),
        Field::new("linked_trace_id", DataType::FixedSizeBinary(16), false),
        Field::new("linked_span_id", DataType::FixedSizeBinary(8), false),
        attrs_field(),
    ])
}

/// The schema of `resources.parquet`.
#[must_use]
pub fn resources_schema() -> Schema {
    Schema::new(vec![
        Field::new("resource_id", DataType::Int32, false),
        attrs_field(),
    ])
}

fn attrs_array<'a>(rows: impl Iterator<Item = &'a Attrs>) -> Result<ArrayRef> {
    let mut keys = StringBuilder::new();
    let mut s = StringBuilder::new();
    let mut i = Int64Builder::new();
    let mut f = Float64Builder::new();
    let mut b = BooleanBuilder::new();
    let mut y = BinaryBuilder::new();
    let mut offsets = vec![0i32];
    for attrs in rows {
        for (k, v) in attrs {
            keys.append_value(k);
            s.append_option(match v {
                AttrValue::String(x) => Some(x.as_str()),
                _ => None,
            });
            i.append_option(match v {
                AttrValue::Int(x) => Some(*x),
                _ => None,
            });
            f.append_option(match v {
                AttrValue::Float(x) => Some(*x),
                _ => None,
            });
            b.append_option(match v {
                AttrValue::Bool(x) => Some(*x),
                _ => None,
            });
            y.append_option(match v {
                AttrValue::Bytes(x) => Some(x.as_slice()),
                _ => None,
            });
        }
        let len = i32::try_from(keys.len())
            .or_else(|_| invalid("more attribute entries than an Int32 offset holds"))?;
        offsets.push(len);
    }
    let value = StructArray::try_new(
        attr_value_fields(),
        vec![
            Arc::new(s.finish()),
            Arc::new(i.finish()),
            Arc::new(f.finish()),
            Arc::new(b.finish()),
            Arc::new(y.finish()),
        ],
        None,
    )?;
    let DataType::Struct(entry_fields) = attrs_entries_field().data_type().clone() else {
        return invalid("attrs entries are a struct");
    };
    let entries = StructArray::try_new(
        entry_fields,
        vec![Arc::new(keys.finish()), Arc::new(value)],
        None,
    )?;
    Ok(Arc::new(MapArray::try_new(
        attrs_entries_field(),
        OffsetBuffer::new(offsets.into()),
        entries,
        None,
        true,
    )?))
}

fn fixed<const N: usize>(rows: impl Iterator<Item = Option<[u8; N]>>) -> Result<ArrayRef> {
    let width = i32::try_from(N).or_else(|_| invalid("id width"))?;
    let mut bld = FixedSizeBinaryBuilder::new(width);
    for r in rows {
        match r {
            Some(v) => bld.append_value(v)?,
            None => bld.append_null(),
        }
    }
    Ok(Arc::new(bld.finish()))
}

fn dict<'a>(rows: impl Iterator<Item = &'a str>) -> ArrayRef {
    let mut bld = StringDictionaryBuilder::<Int32Type>::new();
    for r in rows {
        bld.append_value(r);
    }
    Arc::new(bld.finish())
}

fn i64s(rows: impl Iterator<Item = i64>) -> ArrayRef {
    let mut bld = Int64Builder::new();
    for r in rows {
        bld.append_value(r);
    }
    Arc::new(bld.finish())
}

/// Check every attribute of `attrs` against the inventory: an `acn.*` name must be
/// listed, a listed name must carry its declared type, and no float is NaN or
/// infinite (ADR-13).
fn check_attrs(inv: &Inventory, owner: &str, attrs: &Attrs) -> Result<()> {
    for (k, v) in attrs {
        if let AttrValue::Float(f) = v
            && !f.is_finite()
        {
            return invalid(format!(
                "{owner}: attribute `{k}` is {f}; NaN and infinities are not stored (ADR-13)"
            ));
        }
        match inv.attribute(k) {
            Some(a) if a.ty != v.ty() => {
                return invalid(format!(
                    "{owner}: attribute `{k}` is declared {:?} but carries {:?} (TRC-20)",
                    a.ty,
                    v.ty()
                ));
            }
            Some(_) => {}
            None if k.split_once('.').is_some_and(|(ns, _)| ns == "acn") => {
                return invalid(format!(
                    "{owner}: `{k}` is not listed in acn_attributes.toml (TRC-20)"
                ));
            }
            None => {}
        }
    }
    Ok(())
}

/// The record batches of a trace, checked against the inventory. The trace must
/// already be in stored order ([`Trace::sort`]) with each key once; every event
/// and link must belong to a span of the trace and every span to one of its
/// resources.
pub fn batches(inv: &Inventory, trace: &Trace) -> Result<[RecordBatch; 4]> {
    if !trace.is_sorted() {
        return invalid(
            "the trace is not in stored order, or a key appears twice; call Trace::sort (TRC-25)",
        );
    }
    let mut seen = std::collections::BTreeSet::new();
    for s in &trace.spans {
        if !seen.insert((s.trace_id, s.span_id)) {
            return invalid(format!("span id {:02x?} appears twice", s.span_id));
        }
        check_attrs(inv, &format!("span `{}`", s.name), &s.attrs)?;
        if !trace
            .resources
            .iter()
            .any(|r| r.resource_id == s.resource_id)
        {
            return invalid(format!(
                "span `{}` names resource {} that the trace does not hold",
                s.name, s.resource_id
            ));
        }
    }
    for e in &trace.events {
        if !seen.contains(&(e.trace_id, e.span_id)) {
            return invalid(format!(
                "event `{}` belongs to no span of the trace",
                e.name
            ));
        }
        check_attrs(inv, &format!("event `{}`", e.name), &e.attrs)?;
    }
    for l in &trace.links {
        if !seen.contains(&(l.trace_id, l.span_id)) {
            return invalid("a link belongs to no span of the trace");
        }
        check_attrs(inv, "link", &l.attrs)?;
    }
    for r in &trace.resources {
        check_attrs(inv, "resource", &r.attrs)?;
    }

    let spans = &trace.spans;
    let mut columns: Vec<ArrayRef> = vec![
        fixed(spans.iter().map(|s| Some(s.trace_id)))?,
        fixed(spans.iter().map(|s| Some(s.span_id)))?,
        fixed(spans.iter().map(|s| s.parent_span_id))?,
        dict(spans.iter().map(|s| s.name.as_str())),
        {
            let mut b = Int8Builder::new();
            spans.iter().for_each(|s| b.append_value(s.kind));
            Arc::new(b.finish())
        },
        i64s(spans.iter().map(|s| s.start_ns)),
        i64s(spans.iter().map(|s| s.end_ns)),
        {
            let mut b = Int8Builder::new();
            spans.iter().for_each(|s| b.append_value(s.status_code));
            Arc::new(b.finish())
        },
        {
            let mut b = StringBuilder::new();
            spans
                .iter()
                .for_each(|s| b.append_option(s.status_message.as_deref()));
            Arc::new(b.finish())
        },
        {
            let mut b = Int32Builder::new();
            spans.iter().for_each(|s| b.append_value(s.resource_id));
            Arc::new(b.finish())
        },
        attrs_array(spans.iter().map(|s| &s.attrs))?,
    ];
    for a in inv.promoted() {
        let get = |s: &crate::model::SpanRow| s.attrs.get(&a.name).cloned();
        let col: ArrayRef = match a.ty {
            ValueType::String => {
                let mut b = StringBuilder::new();
                for s in spans {
                    b.append_option(match get(s) {
                        Some(AttrValue::String(v)) => Some(v),
                        _ => None,
                    });
                }
                Arc::new(b.finish())
            }
            ValueType::Int => {
                let mut b = Int64Builder::new();
                for s in spans {
                    b.append_option(match get(s) {
                        Some(AttrValue::Int(v)) => Some(v),
                        _ => None,
                    });
                }
                Arc::new(b.finish())
            }
            ValueType::Float => {
                let mut b = Float64Builder::new();
                for s in spans {
                    b.append_option(match get(s) {
                        Some(AttrValue::Float(v)) => Some(v),
                        _ => None,
                    });
                }
                Arc::new(b.finish())
            }
            ValueType::Bool => {
                let mut b = BooleanBuilder::new();
                for s in spans {
                    b.append_option(match get(s) {
                        Some(AttrValue::Bool(v)) => Some(v),
                        _ => None,
                    });
                }
                Arc::new(b.finish())
            }
            ValueType::Bytes => {
                let mut b = BinaryBuilder::new();
                for s in spans {
                    b.append_option(match get(s) {
                        Some(AttrValue::Bytes(v)) => Some(v),
                        _ => None,
                    });
                }
                Arc::new(b.finish())
            }
        };
        columns.push(col);
    }
    let spans_batch = RecordBatch::try_new(Arc::new(spans_schema(inv)), columns)?;

    let ev = &trace.events;
    let events_batch = RecordBatch::try_new(
        Arc::new(events_schema()),
        vec![
            fixed(ev.iter().map(|e| Some(e.trace_id)))?,
            fixed(ev.iter().map(|e| Some(e.span_id)))?,
            i64s(ev.iter().map(|e| i64::from(e.seq))),
            i64s(ev.iter().map(|e| e.time_ns)),
            dict(ev.iter().map(|e| e.name.as_str())),
            attrs_array(ev.iter().map(|e| &e.attrs))?,
        ],
    )?;

    let ln = &trace.links;
    let links_batch = RecordBatch::try_new(
        Arc::new(links_schema()),
        vec![
            fixed(ln.iter().map(|l| Some(l.trace_id)))?,
            fixed(ln.iter().map(|l| Some(l.span_id)))?,
            i64s(ln.iter().map(|l| i64::from(l.seq))),
            fixed(ln.iter().map(|l| Some(l.linked_trace_id)))?,
            fixed(ln.iter().map(|l| Some(l.linked_span_id)))?,
            attrs_array(ln.iter().map(|l| &l.attrs))?,
        ],
    )?;

    let rs = &trace.resources;
    let resources_batch = RecordBatch::try_new(
        Arc::new(resources_schema()),
        vec![
            {
                let mut b = Int32Builder::new();
                rs.iter().for_each(|r| b.append_value(r.resource_id));
                Arc::new(b.finish())
            },
            attrs_array(rs.iter().map(|r| &r.attrs))?,
        ],
    )?;
    Ok([spans_batch, events_batch, links_batch, resources_batch])
}

/// Write one batch to a new file; an existing file is never replaced.
pub fn write_batch(path: &Path, batch: &RecordBatch) -> Result<()> {
    use std::io::Write as _;
    // One encoder decides the bytes on disk and the bytes `verify_views` compares.
    let bytes = encode(batch)?;
    let mut file: File = OpenOptions::new()
        .write(true)
        .create_new(true)
        .open(path)
        .map_err(io_err(path))?;
    file.write_all(&bytes).map_err(io_err(path))?;
    Ok(())
}

/// Write the four tables of `trace` into `dir` (TRC-22, TRC-25).
pub fn write_trace(dir: &Path, inv: &Inventory, trace: &Trace) -> Result<()> {
    let [spans, events, links, resources] = batches(inv, trace)?;
    write_batch(&dir.join(SPANS), &spans)?;
    write_batch(&dir.join(EVENTS), &events)?;
    write_batch(&dir.join(LINKS), &links)?;
    write_batch(&dir.join(RESOURCES), &resources)?;
    Ok(())
}

fn io_err(path: &Path) -> impl FnOnce(std::io::Error) -> WriteError + '_ {
    move |source| WriteError::Io {
        path: path.display().to_string(),
        source,
    }
}

/// The batches of one table, refused unless its schema is exactly `expected`.
fn read_table(path: &Path, expected: &Schema) -> Result<Vec<RecordBatch>> {
    use parquet::arrow::arrow_reader::ParquetRecordBatchReaderBuilder;
    let file = File::open(path).map_err(io_err(path))?;
    let reader = ParquetRecordBatchReaderBuilder::try_new(file)?.build()?;
    let mut out = Vec::new();
    for batch in reader {
        let batch = batch?;
        if batch.schema().fields() != expected.fields() {
            return invalid(format!(
                "{}: not the expected schema (TRC-25)",
                path.display()
            ));
        }
        out.push(batch);
    }
    Ok(out)
}

/// The attributes of every row of an `attrs` map column.
fn decode_attrs(column: &ArrayRef) -> Result<Vec<Attrs>> {
    use arrow_array::Array as _;
    use arrow_array::cast::AsArray as _;
    use arrow_array::types::{Float64Type, Int64Type};
    let map = column.as_map();
    let keys = map.keys().as_string::<i32>();
    let values = map.values().as_struct();
    let (s, i, f, b, y) = (
        values.column(0).as_string::<i32>(),
        values.column(1).as_primitive::<Int64Type>(),
        values.column(2).as_primitive::<Float64Type>(),
        values.column(3).as_boolean(),
        values.column(4).as_binary::<i32>(),
    );
    let offsets = map.value_offsets();
    let mut rows = Vec::with_capacity(map.len());
    for row in 0..map.len() {
        let (Ok(lo), Ok(hi)) = (
            usize::try_from(offsets[row]),
            usize::try_from(offsets[row + 1]),
        ) else {
            return invalid("negative map offset");
        };
        let mut attrs = Attrs::new();
        for e in lo..hi {
            let set = [
                s.is_valid(e),
                i.is_valid(e),
                f.is_valid(e),
                b.is_valid(e),
                y.is_valid(e),
            ];
            let value = match set {
                [true, false, false, false, false] => AttrValue::String(s.value(e).to_owned()),
                [false, true, false, false, false] => AttrValue::Int(i.value(e)),
                [false, false, true, false, false] => AttrValue::Float(f.value(e)),
                [false, false, false, true, false] => AttrValue::Bool(b.value(e)),
                [false, false, false, false, true] => AttrValue::Bytes(y.value(e).to_vec()),
                _ => return invalid("an attribute value must set exactly one member (TRC-25)"),
            };
            if attrs.insert(keys.value(e).to_owned(), value).is_some() {
                return invalid("an attribute key appears twice in one row");
            }
        }
        rows.push(attrs);
    }
    Ok(rows)
}

fn fixed_at<const N: usize>(column: &ArrayRef, row: usize) -> Result<Option<[u8; N]>> {
    use arrow_array::Array as _;
    use arrow_array::cast::AsArray as _;
    let a = column.as_fixed_size_binary();
    if a.is_null(row) {
        return Ok(None);
    }
    match <[u8; N]>::try_from(a.value(row)) {
        Ok(v) => Ok(Some(v)),
        Err(_) => invalid(format!("an id is not {N} bytes")),
    }
}

fn fixed_req<const N: usize>(column: &ArrayRef, row: usize) -> Result<[u8; N]> {
    fixed_at::<N>(column, row)?.map_or_else(|| invalid("a required id is null"), Ok)
}

fn dict_values(column: &ArrayRef) -> Result<Vec<String>> {
    use arrow_array::cast::AsArray as _;
    use arrow_array::types::Int32Type;
    let d = column.as_dictionary::<Int32Type>();
    let Some(values) = d.values().as_string_opt::<i32>() else {
        return invalid("a dictionary column without string values");
    };
    let mut out = Vec::with_capacity(d.len());
    for k in d.keys().iter() {
        let Some(k) = k.and_then(|k| usize::try_from(k).ok()) else {
            return invalid("a null or negative dictionary key");
        };
        out.push(values.value(k).to_owned());
    }
    Ok(out)
}

fn seq_of(v: i64) -> Result<u32> {
    u32::try_from(v).or_else(|_| invalid("a seq out of range"))
}

/// Read the four tables of a bundle directory back into a [`Trace`] (TRC-35: the
/// views are recomputable from these and from nothing else). Each table must have
/// exactly the schema the writer gives it.
pub fn read_trace(dir: &Path, inv: &Inventory) -> Result<Trace> {
    use crate::model::{EventRow, LinkRow, SpanRow};
    use arrow_array::Array as _;
    use arrow_array::cast::AsArray as _;
    use arrow_array::types::{Int8Type, Int32Type, Int64Type};

    let mut trace = Trace::default();
    for batch in read_table(&dir.join(SPANS), &spans_schema(inv))? {
        let col = |n: &str| batch.column_by_name(n).cloned();
        let (Some(tid), Some(sid), Some(pid), Some(name), Some(kind), Some(start), Some(end)) = (
            col("trace_id"),
            col("span_id"),
            col("parent_span_id"),
            col("name"),
            col("kind"),
            col("start_ns"),
            col("end_ns"),
        ) else {
            return invalid("spans.parquet lacks a base column");
        };
        let (Some(status), Some(message), Some(res), Some(attrs)) = (
            col("status_code"),
            col("status_message"),
            col("resource_id"),
            col("attrs"),
        ) else {
            return invalid("spans.parquet lacks a base column");
        };
        let names = dict_values(&name)?;
        let attrs = decode_attrs(&attrs)?;
        let message = message.as_string::<i32>();
        for row in 0..batch.num_rows() {
            trace.spans.push(SpanRow {
                trace_id: fixed_req::<16>(&tid, row)?,
                span_id: fixed_req::<8>(&sid, row)?,
                parent_span_id: fixed_at::<8>(&pid, row)?,
                name: names[row].clone(),
                kind: kind.as_primitive::<Int8Type>().value(row),
                start_ns: start.as_primitive::<Int64Type>().value(row),
                end_ns: end.as_primitive::<Int64Type>().value(row),
                status_code: status.as_primitive::<Int8Type>().value(row),
                status_message: message.is_valid(row).then(|| message.value(row).to_owned()),
                resource_id: res.as_primitive::<Int32Type>().value(row),
                attrs: attrs[row].clone(),
            });
        }
    }
    for batch in read_table(&dir.join(EVENTS), &events_schema())? {
        let c = batch.columns();
        let names = dict_values(&c[4])?;
        let attrs = decode_attrs(&c[5])?;
        for row in 0..batch.num_rows() {
            trace.events.push(EventRow {
                trace_id: fixed_req::<16>(&c[0], row)?,
                span_id: fixed_req::<8>(&c[1], row)?,
                seq: seq_of(c[2].as_primitive::<Int64Type>().value(row))?,
                time_ns: c[3].as_primitive::<Int64Type>().value(row),
                name: names[row].clone(),
                attrs: attrs[row].clone(),
            });
        }
    }
    for batch in read_table(&dir.join(LINKS), &links_schema())? {
        let c = batch.columns();
        for (row, attrs) in decode_attrs(&c[5])?.into_iter().enumerate() {
            trace.links.push(LinkRow {
                trace_id: fixed_req::<16>(&c[0], row)?,
                span_id: fixed_req::<8>(&c[1], row)?,
                seq: seq_of(c[2].as_primitive::<Int64Type>().value(row))?,
                linked_trace_id: fixed_req::<16>(&c[3], row)?,
                linked_span_id: fixed_req::<8>(&c[4], row)?,
                attrs,
            });
        }
    }
    trace.resources = read_resources(&dir.join(RESOURCES))?;
    Ok(trace)
}

/// Read one derived view of a bundle directory (TRC-31..34) back as Arrow batches.
/// The file must have exactly the schema `views.toml` gives the view (TRC-37).
pub fn read_view(dir: &Path, view: &crate::schema::View) -> Result<Vec<RecordBatch>> {
    let schema =
        crate::ingest::view_schema(view).map_err(|e| WriteError::Invalid(e.to_string()))?;
    read_table(&dir.join(&view.file), &schema)
}

/// Read `resources.parquet` back (TRC-19 checks in `bundle::verify`). A file whose
/// schema is not exactly [`resources_schema`] is refused.
pub fn read_resources(path: &Path) -> Result<Vec<crate::model::ResourceRow>> {
    use arrow_array::cast::AsArray as _;
    use arrow_array::types::Int32Type;
    let mut rows = Vec::new();
    for batch in read_table(path, &resources_schema())? {
        let ids = batch.column(0).as_primitive::<Int32Type>();
        let attrs = decode_attrs(batch.column(1))?;
        for (row, attrs) in attrs.into_iter().enumerate() {
            rows.push(crate::model::ResourceRow {
                resource_id: ids.value(row),
                attrs,
            });
        }
    }
    Ok(rows)
}

/// Encode one batch as a Parquet file in memory, with the writer settings of TRC-25.
pub fn encode(batch: &RecordBatch) -> Result<Vec<u8>> {
    let mut out = Vec::new();
    let mut w = ArrowWriter::try_new(&mut out, batch.schema(), Some(writer_properties()?))?;
    w.write(batch)?;
    w.close()?;
    Ok(out)
}
