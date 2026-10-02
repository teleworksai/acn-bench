//! TRC-25: `spans.parquet` and its siblings are OTLP-shaped, carry a typed column
//! per promoted attribute, hold rows in their stated order, and are written with the
//! settings fixed in the schema module.

#![allow(clippy::unwrap_used, clippy::expect_used, clippy::panic)] // CON-19: tests are exempt

use std::fs::File;

use acn_trace::fixture::{self, FixtureRun};
use acn_trace::identity::Digest;
use acn_trace::model::{AttrValue, Trace};
use acn_trace::parquet_io::{self, EVENTS, LINKS, RESOURCES, SPANS};
use acn_trace::schema;
use arrow_array::cast::AsArray as _;
use arrow_array::types::{Float64Type, Int64Type};
use arrow_array::{Array as _, ArrayAccessor as _, RecordBatch};
use parquet::arrow::arrow_reader::ParquetRecordBatchReaderBuilder;
use parquet::basic::Compression;
use parquet::file::reader::{FileReader as _, SerializedFileReader};

fn trace() -> Trace {
    fixture::session(&FixtureRun {
        run_id: "r".into(),
        seed: 11,
        replicate: 0,
        engine_hash: Digest::of(b"engine"),
        build_hash: Digest::of(b"build"),
    })
    .unwrap()
}

fn written(t: &Trace) -> tempfile::TempDir {
    let dir = tempfile::tempdir().unwrap();
    parquet_io::write_trace(dir.path(), &schema::inventory().unwrap(), t).unwrap();
    dir
}

fn read(path: &std::path::Path) -> RecordBatch {
    let reader = ParquetRecordBatchReaderBuilder::try_new(File::open(path).unwrap())
        .unwrap()
        .build()
        .unwrap();
    let batches: Vec<RecordBatch> = reader.map(Result::unwrap).collect();
    arrow_select_concat(&batches)
}

fn arrow_select_concat(batches: &[RecordBatch]) -> RecordBatch {
    assert_eq!(batches.len(), 1, "fixture tables fit one batch");
    batches[0].clone()
}

/// Cites: TRC-25
#[test]
fn spans_have_the_otlp_columns_then_one_typed_column_per_promoted_attribute() {
    let inv = schema::inventory().unwrap();
    let dir = written(&trace());
    let batch = read(&dir.path().join(SPANS));
    let names: Vec<String> = batch
        .schema()
        .fields()
        .iter()
        .map(|f| f.name().clone())
        .collect();
    let base = [
        "trace_id",
        "span_id",
        "parent_span_id",
        "name",
        "kind",
        "start_ns",
        "end_ns",
        "status_code",
        "status_message",
        "resource_id",
        "attrs",
    ];
    assert_eq!(&names[..base.len()], base);
    let promoted: Vec<String> = inv.promoted().map(|a| a.column_name()).collect();
    assert_eq!(&names[base.len()..], promoted, "in inventory order");
    for a in inv.promoted() {
        let f = batch
            .schema()
            .field_with_name(&a.column_name())
            .unwrap()
            .clone();
        assert!(f.is_nullable(), "{} is nullable", a.name);
    }
    let s = batch.schema();
    assert_eq!(
        format!("{:?}", s.field_with_name("trace_id").unwrap().data_type()),
        "FixedSizeBinary(16)"
    );
    assert_eq!(
        format!("{:?}", s.field_with_name("span_id").unwrap().data_type()),
        "FixedSizeBinary(8)"
    );
    assert!(s.field_with_name("parent_span_id").unwrap().is_nullable());
    assert_eq!(
        format!("{:?}", s.field_with_name("name").unwrap().data_type()),
        "Dictionary(Int32, Utf8)"
    );
}

/// Cites: TRC-25
#[test]
fn promoted_columns_hold_the_attribute_and_are_null_where_it_is_absent() {
    let t = trace();
    let dir = written(&t);
    let batch = read(&dir.path().join(SPANS));
    let names = batch
        .column_by_name("name")
        .unwrap()
        .as_dictionary::<arrow_array::types::Int32Type>();
    let names = names.downcast_dict::<arrow_array::StringArray>().unwrap();
    let ttft = batch
        .column_by_name("acn_call_ttft_ms")
        .unwrap()
        .as_primitive::<Float64Type>();
    let index = batch
        .column_by_name("acn_call_index")
        .unwrap()
        .as_primitive::<Int64Type>();
    let mut chats = 0;
    for row in 0..batch.num_rows() {
        if names.value(row) == "chat" {
            chats += 1;
            assert!((ttft.value(row) - 12.5).abs() < f64::EPSILON);
            assert!(!index.is_null(row));
        } else {
            assert!(ttft.is_null(row), "absent, never zero");
            assert!(index.is_null(row));
        }
    }
    assert_eq!(chats, 3);
}

/// Cites: TRC-25
#[test]
fn attribute_values_are_a_union_with_exactly_one_member_set() {
    let dir = written(&trace());
    let batch = read(&dir.path().join(SPANS));
    let map = batch.column_by_name("attrs").unwrap().as_map();
    let values = map.values().as_struct();
    let members: Vec<&str> = values.fields().iter().map(|f| f.name().as_str()).collect();
    assert_eq!(members, schema::parquet::ATTR_MEMBERS);
    for row in 0..values.len() {
        let set = values.columns().iter().filter(|c| !c.is_null(row)).count();
        assert_eq!(set, 1, "entry {row}");
    }
    // Keys within a span are in bytewise order.
    let keys = map.keys().as_string::<i32>();
    for w in map.value_offsets().windows(2) {
        let (a, b) = (
            usize::try_from(w[0]).unwrap(),
            usize::try_from(w[1]).unwrap(),
        );
        for i in a + 1..b {
            assert!(keys.value(i - 1) < keys.value(i));
        }
    }
}

/// Cites: TRC-25
#[test]
fn rows_are_in_start_trace_span_order() {
    let dir = written(&trace());
    let batch = read(&dir.path().join(SPANS));
    let start = batch
        .column_by_name("start_ns")
        .unwrap()
        .as_primitive::<Int64Type>();
    let tid = batch
        .column_by_name("trace_id")
        .unwrap()
        .as_fixed_size_binary();
    let sid = batch
        .column_by_name("span_id")
        .unwrap()
        .as_fixed_size_binary();
    for r in 1..batch.num_rows() {
        let prev = (start.value(r - 1), tid.value(r - 1), sid.value(r - 1));
        let cur = (start.value(r), tid.value(r), sid.value(r));
        assert!(prev < cur, "row {r}");
    }
    assert_eq!(start.value(0), 0, "sim time starts at 0 (TRC-26)");
}

/// Cites: TRC-25
#[test]
fn the_writer_settings_are_the_ones_fixed_in_the_schema_module() {
    let dir = written(&trace());
    for table in [SPANS, EVENTS, LINKS, RESOURCES] {
        let r = SerializedFileReader::new(File::open(dir.path().join(table)).unwrap()).unwrap();
        let meta = r.metadata().file_metadata();
        assert_eq!(meta.created_by(), Some(schema::parquet::CREATED_BY));
        assert_eq!(meta.version(), 1);
        let kv: Vec<&str> = meta
            .key_value_metadata()
            .map(|v| v.iter().map(|k| k.key.as_str()).collect())
            .unwrap_or_default();
        assert!(
            kv.iter().all(|k| *k == "ARROW:schema"),
            "no metadata but the Arrow schema, so no wall-clock time: {kv:?}"
        );
        for rg in r.metadata().row_groups() {
            for c in rg.columns() {
                assert!(matches!(c.compression(), Compression::ZSTD(_)), "{table}");
            }
        }
    }
    let r = SerializedFileReader::new(File::open(dir.path().join(SPANS)).unwrap()).unwrap();
    let col = r.metadata().row_group(0).column(5); // start_ns
    assert!(col.statistics().is_some(), "statistics on");
    assert_eq!(schema::parquet::ZSTD_LEVEL, 3);
    assert_eq!(schema::parquet::ROW_GROUP_ROWS, 65_536);
}

/// Cites: TRC-25
#[test]
fn row_groups_split_at_the_fixed_row_count() {
    let mut t = trace();
    let template = t.spans[0].clone();
    t.spans.clear();
    for i in 0..70_000i64 {
        let mut s = template.clone();
        s.start_ns = i;
        s.span_id = (u64::try_from(i).unwrap() + 1).to_be_bytes();
        s.parent_span_id = None;
        t.spans.push(s);
    }
    t.events.clear();
    t.links.clear();
    t.sort();
    let dir = written(&t);
    let r = SerializedFileReader::new(File::open(dir.path().join(SPANS)).unwrap()).unwrap();
    let rows: Vec<i64> = r
        .metadata()
        .row_groups()
        .iter()
        .map(|g| g.num_rows())
        .collect();
    assert_eq!(rows, [65_536, 70_000 - 65_536]);
}

/// Cites: TRC-25, TRC-20
#[test]
fn unlisted_names_mistyped_values_and_unsorted_tables_are_refused() {
    let inv = schema::inventory().unwrap();
    let dir = tempfile::tempdir().unwrap();

    let mut t = trace();
    t.spans[0]
        .attrs
        .insert("acn.not_a_name".into(), AttrValue::Int(1));
    assert!(
        parquet_io::batches(&inv, &t).is_err(),
        "an unlisted acn.* name"
    );

    let mut t = trace();
    let chat = t.spans.iter_mut().find(|s| s.name == "chat").unwrap();
    chat.attrs
        .insert("acn.call.ttft_ms".into(), AttrValue::Int(12));
    assert!(
        parquet_io::batches(&inv, &t).is_err(),
        "a float attribute given an int"
    );

    let mut t = trace();
    t.spans.reverse();
    assert!(parquet_io::batches(&inv, &t).is_err(), "rows out of order");

    let mut t = trace();
    let dup = t.spans[1].clone();
    t.spans.push(dup);
    t.sort();
    assert!(parquet_io::batches(&inv, &t).is_err(), "a span id twice");

    // An existing file is never replaced.
    let t = trace();
    parquet_io::write_trace(dir.path(), &inv, &t).unwrap();
    assert!(parquet_io::write_trace(dir.path(), &inv, &t).is_err());
}

/// Cites: TRC-25, TRC-20
#[test]
fn links_and_events_are_checked_and_must_belong_to_a_span() {
    use acn_trace::model::{EventRow, LinkRow};
    let inv = schema::inventory().unwrap();
    let base = trace();
    let s0 = &base.spans[0];
    let link = LinkRow {
        trace_id: s0.trace_id,
        span_id: s0.span_id,
        seq: 0,
        linked_trace_id: [9; 16],
        linked_span_id: [9; 8],
        attrs: [("acn.not_in_inventory".to_owned(), AttrValue::Int(1))].into(),
    };
    let mut t = base.clone();
    t.links.push(link.clone());
    t.sort();
    assert!(
        parquet_io::batches(&inv, &t).is_err(),
        "an unlisted name on a link"
    );

    let mut t = base.clone();
    t.links.push(LinkRow {
        attrs: Default::default(),
        span_id: [7; 8],
        ..link
    });
    t.sort();
    assert!(parquet_io::batches(&inv, &t).is_err(), "a link to no span");

    let mut t = base.clone();
    t.events.push(EventRow {
        trace_id: s0.trace_id,
        span_id: [7; 8],
        seq: 0,
        time_ns: 0,
        name: "acn.stream.first_token".into(),
        attrs: Default::default(),
    });
    t.sort();
    assert!(
        parquet_io::batches(&inv, &t).is_err(),
        "an event of no span"
    );
}

/// Cites: TRC-25
#[test]
fn a_nan_attribute_is_refused_as_nan_not_as_disorder() {
    let inv = schema::inventory().unwrap();
    let mut t = trace();
    let chat = t.spans.iter_mut().find(|s| s.name == "chat").unwrap();
    chat.attrs
        .insert("acn.call.ttft_ms".into(), AttrValue::Float(f64::NAN));
    let err = parquet_io::batches(&inv, &t).unwrap_err().to_string();
    assert!(err.contains("NaN"), "{err}");
}
