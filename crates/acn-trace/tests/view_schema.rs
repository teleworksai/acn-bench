//! TRC-37: `views.toml` lists every column of every view with type, unit and nullability.
//! (The check of the files on disk against it arrives with the view writer, T02c.)

#![allow(clippy::unwrap_used, clippy::expect_used, clippy::panic)] // CON-19: tests are exempt

use acn_trace::schema::{self, Inventory, Views};

/// Cites: TRC-37, TRC-31, TRC-32, TRC-33, TRC-34, TRC-38
#[test]
fn the_five_views_are_declared_with_the_columns_the_spec_names() {
    let views = schema::views().expect("the embedded view schema is valid");
    let names: Vec<&str> = views.iter().map(|v| v.name.as_str()).collect();
    assert_eq!(names, ["session", "turn", "call", "link", "tool"]);
    let has = |view: &str, col: &str| views.view(view).expect(view).column(col).is_some();
    for (view, cols) in [
        (
            "session",
            &[
                "run_id",
                "turns",
                "calls",
                "duration_ns",
                "input_tokens_total",
                "cache_read_tokens_total",
                "wire_bytes_up_total",
                "wire_bytes_down_total",
                "outcome_counts",
            ][..],
        ),
        (
            "turn",
            &[
                "session_id",
                "turn_index",
                "think_time_before_ns",
                "chain_length",
                "fanout_width",
                "duration_ns",
                "first_useful_result_ns",
                "outcome",
                "network_wait_ns",
                "tool_wait_ns",
                "model_wait_ns",
                "queue_wait_ns",
                "stalls",
                "retries",
                "compaction",
            ][..],
        ),
        (
            "call",
            &[
                "lineage_id",
                "input_tokens",
                "new_input_tokens",
                "output_tokens",
                "stop_reason",
                "duration_ns",
                "preceding_tool_count",
                "preceding_tool_ns",
                "preceding_tool_class",
                "cache_read_tokens",
                "cache_write_tokens",
                "cached_token_ratio",
                "ttft_ns",
                "itl_p50_ns",
                "itl_p99_ns",
                "wire_bytes_up",
                "wire_bytes_down",
                "server_prefill_ns",
                "server_decode_ns",
                "retries",
                "error_class",
            ][..],
        ),
        (
            "link",
            &[
                "call_id",
                "direction",
                "bytes",
                "applied_delay_ns",
                "dropped",
                "reordered",
                "rate_limited_ns",
                "scenario_step",
                "outage_id",
            ][..],
        ),
        (
            "tool",
            &[
                "lineage_id",
                "requesting_call",
                "tool_class",
                "placement",
                "duration_ns",
                "result_bytes",
            ][..],
        ),
    ] {
        for c in cols {
            assert!(has(view, c), "views.toml: view `{view}` lacks column `{c}`");
        }
    }
}

/// Cites: TRC-37
#[test]
fn every_column_states_type_unit_and_nullability_and_units_match_suffixes() {
    let views = schema::views().expect("views");
    for v in views.iter() {
        assert_eq!(v.file, format!("views/{}.parquet", v.name));
        assert!(!v.row_per.is_empty(), "{}", v.name);
        for c in &v.columns {
            if c.name.ends_with("_ns") {
                assert_eq!(
                    (c.ty.as_str(), c.unit.as_str()),
                    ("int64", "ns"),
                    "{}.{}",
                    v.name,
                    c.name
                );
            }
            if c.name.ends_with("_tokens") || c.name.ends_with("_tokens_total") {
                assert_eq!(c.unit, "tokens", "{}.{}", v.name, c.name);
            }
        }
        // What may be absent in a span is nullable in the view: never zero (SPEC 010 §3).
        for nullable in [
            "think_time_before_ns",
            "first_useful_result_ns",
            "queue_wait_ns",
            "ttft_ns",
            "error_class",
            "lineage_id",
            "outage_id",
            "preceding_tool_ns",
        ] {
            if let Some(c) = v.column(nullable) {
                assert!(c.nullable, "{}.{} must be nullable", v.name, nullable);
            }
        }
    }
}

/// Cites: TRC-37
#[test]
fn a_malformed_view_schema_is_rejected() {
    let ok = "schema_version = 1\n[[view]]\nname = \"session\"\nfile = \"views/session.parquet\"\nrow_per = \"acn.session\"\nrequirement = \"TRC-31\"\n[[view.column]]\nname = \"turns\"\ntype = \"int64\"\nunit = \"\"\nnullable = false\n";
    assert!(Views::parse(ok).is_ok());
    for (bad, needle) in [
        (ok.replace("nullable = false\n", ""), "nullable"),
        (
            ok.replace("type = \"int64\"", "type = \"decimal\""),
            "decimal",
        ),
        (
            ok.replace("unit = \"\"\n", "unit = \"\"\nextra = 1\n"),
            "unknown field",
        ),
        (
            format!(
                "{ok}[[view.column]]\nname = \"turns\"\ntype = \"int64\"\nunit = \"\"\nnullable = false\n"
            ),
            "duplicate",
        ),
        (
            ok.replace(
                "file = \"views/session.parquet\"",
                "file = \"elsewhere.parquet\"",
            ),
            "views/session.parquet",
        ),
    ] {
        let err = Views::parse(&bad)
            .expect_err("must be rejected")
            .to_string();
        assert!(err.contains(needle), "expected `{needle}` in: {err}");
    }
}

fn embedded_views_text() -> String {
    let root = std::path::PathBuf::from(env!("CARGO_MANIFEST_DIR"));
    std::fs::read_to_string(root.join("src/schema/views.toml")).expect("views.toml")
}

fn inv() -> Inventory {
    schema::inventory().expect("inventory")
}

/// When a column is a copy of an attribute, the two files must agree. Every mutation
/// below loaded cleanly before the adversarial review of T02a.
///
/// Cites: TRC-37, TRC-42
#[test]
fn view_columns_are_checked_against_the_inventory() {
    let text = embedded_views_text();
    assert!(
        Views::parse_checked(&text, &inv()).is_ok(),
        "the embedded files agree"
    );
    let mutate = |from: &str, to: &str| -> String {
        assert!(text.contains(from), "views.toml lacks `{from}`");
        let bad = text.replacen(from, to, 1);
        Views::parse_checked(&bad, &inv())
            .expect_err("must be rejected")
            .to_string()
    };
    // An optional attribute in a non-nullable column, and the reverse.
    let e = mutate(
        "name = \"input_tokens\"\ntype = \"int64\"\nunit = \"tokens\"\nnullable = true",
        "name = \"input_tokens\"\ntype = \"int64\"\nunit = \"tokens\"\nnullable = false",
    );
    assert!(e.contains("input_tokens") && e.contains("nullable"), "{e}");
    let e = mutate(
        "name = \"requesting_call\"\ntype = \"int64\"\nunit = \"\"\nnullable = false",
        "name = \"requesting_call\"\ntype = \"int64\"\nunit = \"\"\nnullable = true",
    );
    assert!(
        e.contains("requesting_call") && e.contains("nullable"),
        "{e}"
    );
    // A type that cannot hold the attribute.
    let e = mutate(
        "name = \"stop_reason\"\ntype = \"utf8\"",
        "name = \"stop_reason\"\ntype = \"int64\"",
    );
    assert!(e.contains("stop_reason") && e.contains("type"), "{e}");
    // A row source that is not a span, and a source naming no attribute at all.
    let e = mutate("row_per = \"execute_tool\"", "row_per = \"banana\"");
    assert!(e.contains("banana"), "{e}");
    let e = mutate("source = \"acn.tool.class\"", "source = \"acn.tool.klass\"");
    assert!(e.contains("acn.tool.klass"), "{e}");
    // Message content never reaches a view (TRC-42).
    let e = mutate(
        "source = \"gen_ai.tool.name\"",
        "source = \"gen_ai.input.messages\"",
    );
    assert!(e.contains("content"), "{e}");
    // Two views cannot share a file.
    let e = mutate(
        "name = \"tool\"\nfile = \"views/tool.parquet\"",
        "name = \"tool\"\nfile = \"views/link.parquet\"",
    );
    assert!(e.contains("views/tool.parquet"), "{e}");
}

/// Cites: TRC-31, TRC-32, TRC-33
#[test]
fn the_columns_the_review_added_or_tightened() {
    let views = schema::views().expect("views");
    let col = |v: &str, c: &str| {
        views
            .view(v)
            .expect(v)
            .column(c)
            .unwrap_or_else(|| panic!("{v}.{c}"))
            .clone()
    };
    assert!(
        !col("turn", "fanout_depth").nullable,
        "Appendix A asks for fan-out degree and depth"
    );
    assert!(!col("session", "calls_with_usage").nullable);
    assert!(
        col("session", "input_tokens_total")
            .source
            .contains("null when any call")
    );
    assert!(!col("call", "new_input_tokens_method").nullable);
    assert!(
        col("call", "ttft_ns").source.contains("streamed = false"),
        "a non-streamed call has a ttft too (TRC-12)"
    );
}
