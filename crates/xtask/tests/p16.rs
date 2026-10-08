//! `cargo xtask p16-compare` (SPEC 140 P16-12): one run's records from several
//! targets, compared in build-neutral form; differences are reported, not
//! failures.

#![allow(clippy::unwrap_used, clippy::expect_used, clippy::panic)] // CON-19: tests are exempt

use serde_json::{Value, json};

fn record(target: &str) -> Value {
    json!({
        "ok": true,
        "run_id": "a".repeat(64),
        "target": target,
        "build_hash": format!("build of {target}"),
        "files": {"spans.parquet": "1", "events.parquet": "2"},
        "resources": ["ResourceRow { resource_id: 0 }"],
        "manifest": {"seed": "1", "files": {"spans.parquet": "1"}},
    })
}

fn compare(records: &[Value]) -> Result<xtask::p16::Report, xtask::Error> {
    let dir = tempfile::tempdir().unwrap();
    let paths: Vec<_> = records
        .iter()
        .enumerate()
        .map(|(i, r)| {
            let p = dir.path().join(format!("{i}.json"));
            std::fs::write(&p, r.to_string()).unwrap();
            p
        })
        .collect();
    xtask::p16::run(&paths, 3)
}

/// Cites: P16-12
#[test]
fn records_that_agree_are_identical_and_differences_are_reported_not_failed() {
    let r = compare(&[record("x86"), record("arm"), record("mac")]).unwrap();
    assert!(r.ok && r.identical);
    assert_eq!(r.targets, ["x86", "arm", "mac"]);
    // The build fields differ by design and are not compared.
    let mut arm = record("arm");
    arm["files"]["spans.parquet"] = "9".into();
    arm["manifest"]["files"]["spans.parquet"] = "9".into();
    let mut mac = record("mac");
    mac["resources"] = json!(["ResourceRow { resource_id: 1 }"]);
    let r = compare(&[record("x86"), arm, mac]).unwrap();
    assert!(r.ok, "a difference is evidence, not a failure (CON-31)");
    assert!(!r.identical);
    assert_eq!(r.differ["arm"], ["manifest.files", "spans.parquet"]);
    assert_eq!(r.differ["mac"], ["resources"]);
    assert!(!r.differ.contains_key("x86"));
    // Fewer records than expected: compared, and not identical.
    let r = compare(&[record("x86"), record("arm")]).unwrap();
    assert!(r.ok && !r.identical);
    assert_eq!((r.expected, r.records), (3, 2));
}

/// Cites: P16-12
#[test]
fn records_of_another_run_or_of_no_bundle_are_refused() {
    let mut other = record("arm");
    other["run_id"] = "b".repeat(64).into();
    assert!(compare(&[record("x86"), other]).is_err());
    assert!(
        compare(&[
            record("x86"),
            json!({"ok": false, "code": "bundle_invalid"})
        ])
        .is_err()
    );
    assert!(compare(&[]).is_err());
    // Two records of one target, or a record without one.
    assert!(compare(&[record("x86"), record("x86")]).is_err());
    let mut bare = record("arm");
    bare["target"] = Value::Null;
    assert!(compare(&[record("x86"), bare]).is_err());
}

/// Cites: P16-12
#[test]
fn ci_regenerates_on_every_target_and_compares_them() {
    let ci = std::fs::read_to_string(concat!(
        env!("CARGO_MANIFEST_DIR"),
        "/../../.github/workflows/ci.yml"
    ))
    .unwrap();
    let (_, rest) = ci.split_once("\n  p16-target:").unwrap();
    let (target, compare) = rest.split_once("\n  p16-compare:").unwrap();
    assert!(target.contains("macos-latest, ubuntu-latest, ubuntu-24.04-arm"));
    assert!(target.contains("run: tools/p16-target.sh"));
    assert!(target.contains("name: p16-record-${{ matrix.os }}"));
    assert!(compare.contains("needs: p16-target"));
    assert!(compare.contains("if: ${{ !cancelled() }}"));
    assert!(compare.contains("cargo xtask p16-compare --expect 3"));
    assert!(compare.contains("name: p16-comparison"));
}
