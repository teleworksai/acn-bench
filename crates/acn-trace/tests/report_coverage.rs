//! TRC-36: every Appendix A key of SPEC 010 is mapped, and nothing else is; every
//! named view column and attribute exists in the schema.

#![allow(clippy::unwrap_used, clippy::expect_used, clippy::panic)] // CON-19: tests are exempt

use std::path::Path;

use acn_trace::coverage::{self, Coverage};
use acn_trace::schema;

fn root() -> &'static Path {
    Path::new(concat!(env!("CARGO_MANIFEST_DIR"), "/../.."))
}

fn keys() -> Vec<String> {
    coverage::appendix_a_keys(&std::fs::read_to_string(root().join(coverage::SPEC_FILE)).unwrap())
        .unwrap()
}

fn mapping() -> String {
    std::fs::read_to_string(root().join(coverage::COVERAGE_FILE)).unwrap()
}

fn parse(text: &str) -> Result<Coverage, coverage::CoverageError> {
    Coverage::parse(
        text,
        &keys(),
        &schema::views().unwrap(),
        &schema::inventory().unwrap(),
    )
}

/// Cites: TRC-36
#[test]
fn every_appendix_c_parameter_and_section_3_5_field_is_mapped() {
    let k = keys();
    assert_eq!(k.len(), 29, "SPEC 010 Appendix A lists 29 keys");
    assert_eq!(k.first().map(String::as_str), Some("h.session_id"));
    let cov = parse(&mapping()).unwrap();
    assert_eq!(cov.entries.len(), k.len());
    assert_eq!(
        cov.entries
            .iter()
            .map(|e| e.key.clone())
            .collect::<Vec<_>>(),
        k,
        "rendered in Appendix A order"
    );
    let page = cov.page();
    for key in &k {
        assert!(page.contains(&format!("`{key}`")), "{key} is on the page");
    }
}

/// Cites: TRC-36
#[test]
fn an_unmapped_key_an_unknown_key_and_unknown_targets_fail() {
    let text = mapping();
    let drop_entry = |key: &str| -> String {
        let marker = format!("[[key]]\nkey = \"{key}\"\n");
        let start = text.find(&marker).unwrap();
        let end = text[start + marker.len()..]
            .find("[[key]]")
            .map_or(text.len(), |e| start + marker.len() + e);
        format!("{}{}", &text[..start], &text[end..])
    };
    let cases = [
        (drop_entry("h.ttft"), "has no entry"),
        (
            format!(
                "{text}\n[[key]]\nkey = \"z.not_in_appendix\"\nkind = \"not_recorded\"\nreason = \"r\"\nside_channel = \"s\"\n"
            ),
            "not an Appendix A key",
        ),
        (
            text.replacen("column = \"ttft_ns\"", "column = \"ttft_ms\"", 1),
            "does not define",
        ),
        (
            text.replacen("view = \"session\"", "view = \"sessions\"", 1),
            "does not define",
        ),
        (
            text.replacen(
                "attribute = \"acn.fanout.parent_call\"",
                "attribute = \"acn.harness.knobs\"",
                1,
            ),
            "not list as promoted",
        ),
        (
            text.replacen(
                "columns = [\"tool.duration_ns\"",
                "columns = [\"tool.duration_ms\", \"tool.duration_ns\"",
                1,
            ),
            "does not define",
        ),
        (
            text.replacen(
                "kind = \"attribute\"\nattribute",
                "kind = \"column\"\nattribute",
                1,
            ),
            "must carry `view`",
        ),
        (
            format!(
                "{text}\n[[key]]\nkey = \"h.ttft\"\nkind = \"column\"\nview = \"call\"\ncolumn = \"ttft_ns\"\n"
            ),
            "mapped twice",
        ),
        (
            text.replacen(
                "columns = [\"tool.duration_ns\", \"tool.tool_class\"]",
                "columns = [\"tool.duration_ns\"]",
                1,
            ),
            "which `columns` does not list",
        ),
        (
            text.replacen(
                "columns = [\"tool.duration_ns\"",
                "columns = [\"nodot\", \"tool.duration_ns\"",
                1,
            ),
            "is not `view.column`",
        ),
        (
            text.replacen("report_version = \"v1.2\"", "report_version = \"\"", 1),
            "report_version",
        ),
        (
            text.replacen("kind = \"column\"\nview", "kind = \"column\"\nviw", 1),
            "unknown field",
        ),
    ];
    for (bad, needle) in cases {
        let err = parse(&bad).unwrap_err().to_string();
        assert!(err.contains(needle), "expected `{needle}`, got: {err}");
    }
}

/// Cites: TRC-36
#[test]
fn a_spec_without_appendix_a_keys_is_an_error() {
    assert!(
        coverage::appendix_a_keys("# SPEC\n")
            .unwrap_err()
            .to_string()
            .contains("no `## Appendix A`")
    );
    assert!(coverage::appendix_a_keys("## Appendix A\n\nnothing\n").is_err());
}
