//! HAR-60, HAR-61: the workload format, strict at every level, and the smoke
//! workload the crate's tests run.

#![allow(clippy::unwrap_used, clippy::expect_used, clippy::panic)] // CON-19: tests are exempt

mod common;

use acn_harness::workload::Workload;

/// Cites: HAR-61, HAR-60
#[test]
fn the_smoke_workload_loads_and_exercises_every_clause() {
    let text = common::smoke();
    let w = Workload::parse(text.as_bytes()).unwrap();
    assert_eq!(w.hash, acn_trace::identity::Digest::of(text.as_bytes()));
    assert_eq!(
        w.tasks.iter().map(|t| t.id.as_str()).collect::<Vec<_>>(),
        ["edit", "fanout"]
    );
    let classes: Vec<&str> = w.tools.iter().map(|t| t.class.as_str()).collect();
    assert!(classes.contains(&"subagent") && classes.contains(&"file"));
    let edit = &w.tasks[0];
    assert!(edit.turns.iter().any(|t| !t.updates.is_empty()), "backfill");
    assert!(
        edit.turns.iter().any(|t| t.deadline_ms.is_some()),
        "deadline"
    );
    assert!(
        edit.turns.iter().any(|t| !t.expect_tools.is_empty()),
        "checker"
    );
    assert!(edit.tools.len() >= 2, "an order to keep or shuffle");
}

/// Cites: HAR-60
#[test]
fn a_workload_states_everything_and_names_only_what_exists() {
    let good = common::smoke();
    assert!(Workload::parse(good.as_bytes()).is_ok());
    let cases = [
        (
            good.replace("schema_version = 1", "schema_version = 2"),
            "schema_version",
        ),
        (good.replace("max_tokens = 64\n", ""), "missing field"),
        (
            good.replace("stream = true", "stream = true\nseed = 3"),
            "unknown field",
        ),
        (
            good.replace("temperature = 0.0", "temperature = 2.5"),
            "temperature",
        ),
        (
            good.replace("class = \"search\"", "class = \"database\""),
            "class",
        ),
        (good.replace("width = 2\n", ""), "width"),
        (
            good.replace(
                "tools = [\"read_file\"]\nmax_calls",
                "tools = [\"delegate\"]\nmax_calls",
            ),
            "cannot spawn",
        ),
        (
            good.replace(
                "tools = [\"read_file\", \"grep\"]",
                "tools = [\"read_file\", \"cat\"]",
            ),
            "unknown or listed twice",
        ),
        (
            good.replace("expect_tools = [\"delegate\"]", "expect_tools = [\"grep\"]"),
            "not one of the task's tools",
        ),
        (
            good.replace(
                "result_bytes = { min = 600, max = 1200 }",
                "result_bytes = { min = 1200, max = 600 }",
            ),
            "exceeds max",
        ),
        (
            good.replace("result_bytes = { min = 200, max = 500 }\n", ""),
            "result_bytes",
        ),
        (
            good.replace("deadline_ms = 600_000", "deadline_ms = 0"),
            "deadline",
        ),
        (good.replace("id = \"fanout\"", "id = \"edit\""), "unique"),
        (
            good.replace("max_calls_per_turn = 6", "max_calls_per_turn = 0"),
            "positive",
        ),
    ];
    for (bad, needle) in cases {
        assert_ne!(bad, good, "the case `{needle}` changed nothing");
        let err = Workload::parse(bad.as_bytes()).unwrap_err().to_string();
        assert!(err.contains(needle), "expected `{needle}`, got: {err}");
    }
}

/// Cites: HAR-60, CON-27
#[test]
fn workloads_are_hashed_as_working_tree_bytes() {
    let attrs =
        std::fs::read_to_string(concat!(env!("CARGO_MANIFEST_DIR"), "/../../.gitattributes"))
            .unwrap();
    assert!(attrs.lines().any(|l| l.trim() == "workloads/** -text"));
}
