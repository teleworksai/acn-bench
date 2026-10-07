//! Tests for `cargo xtask env-hash` (CON-7): the hash over the frozen set, its
//! recorded value, and the `--check` gate.

#![allow(clippy::unwrap_used, clippy::expect_used, clippy::panic)] // CON-19: tests are exempt

mod common;

use std::fs;
use std::path::Path;

use common::{repo_root, strings, xtask_at};

const FROZEN_FILES: &[&str] = &[
    "hypotheses/p4.toml",
    "scenarios/measured/walk/trace.parquet",
    "crates/acn-hyp/src/lib.rs",
    "crates/acn-attrib/src/core/mod.rs",
    "crates/acn-attrib/src/lib.rs",
    "crates/acn-attrib/Cargo.toml",
    "crates/acn-trace/src/schema/mod.rs",
    "crates/acn-trace/src/ingest/mod.rs",
];

const OUTSIDE_FILES: &[&str] = &[
    "crates/acn-trace/src/lib.rs",
    "crates/acn-trace/src/parquet_io.rs",
    "scenarios/synthetic/a.toml",
    "lab/hypotheses/p17-a2a.toml",
    "specs/000-constitution.md",
];

fn write(root: &Path, rel: &str, body: &str) {
    let p = root.join(rel);
    fs::create_dir_all(p.parent().expect("parent")).expect("mkdir");
    fs::write(p, body).expect("write");
}

fn frozen_fixture() -> tempfile::TempDir {
    let dir = tempfile::tempdir().expect("tempdir");
    for f in FROZEN_FILES.iter().chain(OUTSIDE_FILES) {
        write(dir.path(), f, &format!("content of {f}\n"));
    }
    // An empty placeholder is the one thing the walk skips.
    write(dir.path(), "scenarios/measured/.gitkeep", "");
    dir
}

fn hash_of(root: &Path) -> String {
    let run = xtask_at(root, &["env-hash"]);
    assert!(run.ok(), "{}", run.json);
    run.json["env_hash"].as_str().expect("env_hash").to_owned()
}

/// Cites: CON-7
#[test]
fn hash_covers_exactly_the_frozen_set() {
    let dir = frozen_fixture();
    let run = xtask_at(dir.path(), &["env-hash"]);
    assert!(run.ok(), "{}", run.json);
    let mut files: Vec<String> = run.json["files"]
        .as_array()
        .expect("files")
        .iter()
        .map(|f| f["path"].as_str().expect("path").to_owned())
        .collect();
    files.sort();
    let mut expected: Vec<String> = FROZEN_FILES.iter().map(|s| (*s).to_owned()).collect();
    expected.sort();
    assert_eq!(files, expected);
}

/// Cites: CON-7
#[test]
fn hash_is_deterministic_and_hex_blake3() {
    let dir = frozen_fixture();
    let a = hash_of(dir.path());
    let b = hash_of(dir.path());
    assert_eq!(a, b);
    assert_eq!(a.len(), 64, "blake3 hex is 64 chars: {a}");
    assert!(a.chars().all(|c| c.is_ascii_hexdigit()));
}

/// Cites: CON-7
#[test]
fn hash_changes_only_when_a_frozen_file_changes() {
    let dir = frozen_fixture();
    let base = hash_of(dir.path());
    for f in OUTSIDE_FILES {
        write(dir.path(), f, "changed\n");
    }
    assert_eq!(
        hash_of(dir.path()),
        base,
        "non-frozen edits must not move the hash"
    );
    write(dir.path(), "hypotheses/p4.toml", "changed\n");
    let moved = hash_of(dir.path());
    assert_ne!(moved, base);
    // A new file inside a frozen directory also moves it.
    write(dir.path(), "hypotheses/p99.toml", "new\n");
    assert_ne!(hash_of(dir.path()), moved);
}

/// Cites: CON-7
#[test]
fn check_fails_without_a_record_and_passes_after_write() {
    let dir = frozen_fixture();
    let no_record = xtask_at(dir.path(), &["env-hash", "--check"]);
    assert!(!no_record.ok(), "{}", no_record.json);

    let written = xtask_at(dir.path(), &["env-hash", "--write"]);
    assert!(written.ok(), "{}", written.json);
    assert!(dir.path().join("env-hash.json").is_file());

    let check = xtask_at(dir.path(), &["env-hash", "--check"]);
    assert!(check.ok(), "{}", check.json);
    assert_eq!(check.json["env_hash"], check.json["recorded"]);

    write(dir.path(), "crates/acn-hyp/src/lib.rs", "tampered\n");
    let tampered = xtask_at(dir.path(), &["env-hash", "--check"]);
    assert!(!tampered.ok(), "{}", tampered.json);
    assert_ne!(tampered.json["env_hash"], tampered.json["recorded"]);
}

/// Cites: CON-7
#[test]
fn self_host_the_recorded_hash_matches_the_workspace() {
    let run = xtask_at(&repo_root(), &["env-hash", "--check"]);
    assert!(run.ok(), "{}", run.json);
    let files = strings(&run.json, "files");
    assert!(!files.is_empty(), "the frozen set must not be empty");
}

/// Cites: CON-8
#[test]
fn env_hash_honours_the_json_contract_on_both_outcomes() {
    let dir = frozen_fixture();
    assert_eq!(xtask_at(dir.path(), &["env-hash", "--check"]).code, Some(1));
    assert_eq!(xtask_at(dir.path(), &["env-hash", "--write"]).code, Some(0));
    assert_eq!(xtask_at(dir.path(), &["env-hash", "--check"]).code, Some(0));
}

/// Cites: CON-7
#[cfg(unix)]
#[test]
fn symlinks_inside_the_frozen_set_are_refused() {
    let dir = frozen_fixture();
    std::os::unix::fs::symlink(
        dir.path().join("crates/acn-trace/src/lib.rs"),
        dir.path().join("hypotheses/link.toml"),
    )
    .expect("symlink");
    let run = xtask_at(dir.path(), &["env-hash"]);
    assert!(!run.ok(), "{}", run.json);
    assert!(
        run.json["error"]
            .as_str()
            .expect("error")
            .contains("symlink or special file"),
        "{}",
        run.json
    );
}

/// Cites: CON-7
#[test]
fn check_failure_reports_the_per_file_diff_and_a_hint() {
    let dir = frozen_fixture();
    assert!(xtask_at(dir.path(), &["env-hash", "--write"]).ok());
    write(dir.path(), "hypotheses/p4.toml", "changed\n");
    write(dir.path(), "hypotheses/new.toml", "new\n");
    fs::remove_file(dir.path().join("crates/acn-hyp/src/lib.rs")).expect("rm");
    let run = xtask_at(dir.path(), &["env-hash", "--check"]);
    assert!(!run.ok(), "{}", run.json);
    assert_eq!(
        strings(&run.json["diff"], "changed"),
        vec!["hypotheses/p4.toml"]
    );
    assert_eq!(
        strings(&run.json["diff"], "added"),
        vec!["hypotheses/new.toml"]
    );
    assert_eq!(
        strings(&run.json["diff"], "removed"),
        vec!["crates/acn-hyp/src/lib.rs"]
    );
    assert!(
        run.json["error"]
            .as_str()
            .expect("error")
            .contains("env-change")
    );

    fs::write(dir.path().join("env-hash.json"), "{ not json").expect("write");
    let corrupt = xtask_at(dir.path(), &["env-hash", "--check"]);
    assert!(!corrupt.ok());
    assert!(
        corrupt.json["error"]
            .as_str()
            .expect("error")
            .contains("env-hash.json")
    );
}

/// Cites: CON-7
#[test]
fn a_placeholder_with_content_is_frozen_content() {
    // `#[path = ".gitkeep"] mod x;` or `include!` can turn any file name into code.
    let dir = frozen_fixture();
    let base = hash_of(dir.path());
    write(
        dir.path(),
        "crates/acn-hyp/src/.gitkeep",
        "pub const THRESHOLD: f64 = 0.05;\n",
    );
    let with_code = hash_of(dir.path());
    assert_ne!(with_code, base);
    write(
        dir.path(),
        "crates/acn-hyp/src/.gitkeep",
        "pub const THRESHOLD: f64 = 0.99;\n",
    );
    assert_ne!(hash_of(dir.path()), with_code);

    write(dir.path(), "hypotheses/.DS_Store", "finder junk");
    let run = xtask_at(dir.path(), &["env-hash"]);
    assert!(!run.ok(), "{}", run.json);
    assert!(
        run.json["error"]
            .as_str()
            .expect("error")
            .contains(".DS_Store")
    );
}

/// Cites: CON-7
#[test]
fn check_verifies_the_whole_record_not_only_the_top_level_hash() {
    let dir = frozen_fixture();
    assert!(xtask_at(dir.path(), &["env-hash", "--write"]).ok());
    write(dir.path(), "hypotheses/p4.toml", "changed\n");
    let new_hash = hash_of(dir.path());

    // Forge only the top-level value; every per-file hash still says "nothing moved".
    let record = dir.path().join("env-hash.json");
    let mut json: serde_json::Value =
        serde_json::from_str(&fs::read_to_string(&record).expect("read")).expect("json");
    json["env_hash"] = serde_json::Value::String(new_hash);
    fs::write(&record, serde_json::to_string_pretty(&json).expect("ser")).expect("write");

    let run = xtask_at(dir.path(), &["env-hash", "--check"]);
    assert!(!run.ok(), "{}", run.json);
    assert!(
        run.json["error"]
            .as_str()
            .expect("error")
            .contains("inconsistent"),
        "{}",
        run.json
    );
}

/// Cites: CON-7
#[test]
fn a_missing_frozen_directory_or_a_bad_root_is_an_error_not_an_empty_set() {
    let dir = frozen_fixture();
    fs::remove_dir_all(dir.path().join("scenarios/measured")).expect("rm");
    let run = xtask_at(dir.path(), &["env-hash"]);
    assert!(!run.ok(), "{}", run.json);
    assert!(
        run.json["error"]
            .as_str()
            .expect("error")
            .contains("scenarios/measured"),
        "{}",
        run.json
    );

    let nowhere = dir.path().join("does-not-exist");
    let run = xtask_at(&nowhere, &["env-hash"]);
    assert!(!run.ok(), "{}", run.json);
}

/// Cites: CON-7
#[test]
fn write_repairs_a_corrupt_record() {
    let dir = frozen_fixture();
    fs::write(dir.path().join("env-hash.json"), "<<<<<<< HEAD\n{ not json").expect("write");
    assert!(!xtask_at(dir.path(), &["env-hash", "--check"]).ok());
    assert!(
        xtask_at(dir.path(), &["env-hash", "--write"]).ok(),
        "the advised command must work"
    );
    assert!(xtask_at(dir.path(), &["env-hash", "--check"]).ok());
}

// ---- engine_hash (CON-28, ADR-4 amendment 3): the hash runs and verdicts are bound to.

fn engine_hash_of(root: &Path) -> String {
    let run = xtask_at(root, &["env-hash"]);
    assert!(run.ok(), "{}", run.json);
    run.json["engine_hash"]
        .as_str()
        .expect("engine_hash")
        .to_owned()
}

/// The ADR-4 record construction, written out independently of the tool.
fn records_hash(files: &[(String, String)]) -> String {
    let mut hasher = blake3::Hasher::new();
    for (path, hex) in files {
        hasher.update(path.as_bytes());
        hasher.update(b"\0");
        hasher.update(hex.as_bytes());
        hasher.update(b"\n");
    }
    hasher.finalize().to_hex().to_string()
}

/// Cites: CON-28
#[test]
fn engine_hash_is_the_record_hash_of_the_frozen_crate_paths_only() {
    let dir = frozen_fixture();
    let run = xtask_at(dir.path(), &["env-hash"]);
    assert!(run.ok(), "{}", run.json);
    let all: Vec<(String, String)> = run.json["files"]
        .as_array()
        .expect("files")
        .iter()
        .map(|f| {
            (
                f["path"].as_str().expect("path").to_owned(),
                f["blake3"].as_str().expect("blake3").to_owned(),
            )
        })
        .collect();
    let engine: Vec<(String, String)> = all
        .iter()
        .filter(|(p, _)| p.starts_with("crates/"))
        .cloned()
        .collect();
    assert_eq!(engine.len(), 6, "{engine:?}");
    assert_eq!(
        run.json["engine_hash"].as_str(),
        Some(records_hash(&engine).as_str())
    );
    assert_eq!(
        run.json["env_hash"].as_str(),
        Some(records_hash(&all).as_str()),
        "env_hash keeps the ADR-4 construction over the whole set"
    );
    assert_ne!(run.json["engine_hash"], run.json["env_hash"]);
}

/// Freezing another POC's hypothesis, or adding a measured trace, must not change
/// the identity of any existing run: that is the whole point of the second hash.
///
/// Cites: CON-28, ATR-31
#[test]
fn engine_hash_ignores_hypotheses_and_measured_scenarios_and_follows_the_frozen_crates() {
    let dir = frozen_fixture();
    let engine = engine_hash_of(dir.path());
    let env = hash_of(dir.path());

    write(
        dir.path(),
        "hypotheses/p5.toml",
        "a second frozen hypothesis\n",
    );
    write(dir.path(), "hypotheses/p4.toml", "edited\n");
    write(
        dir.path(),
        "scenarios/measured/drive/trace.parquet",
        "new\n",
    );
    assert_eq!(engine_hash_of(dir.path()), engine);
    assert_ne!(
        hash_of(dir.path()),
        env,
        "env_hash still covers them (CON-7)"
    );

    for frozen_code in [
        "crates/acn-hyp/src/lib.rs",
        "crates/acn-attrib/src/core/mod.rs",
        "crates/acn-attrib/src/lib.rs",
        "crates/acn-attrib/Cargo.toml",
        "crates/acn-trace/src/schema/mod.rs",
        "crates/acn-trace/src/ingest/mod.rs",
    ] {
        let before = engine_hash_of(dir.path());
        write(dir.path(), frozen_code, "changed\n");
        assert_ne!(engine_hash_of(dir.path()), before, "{frozen_code}");
    }
    // Run-path code outside the frozen set is build identity (CON-31), not engine identity.
    let before = engine_hash_of(dir.path());
    write(dir.path(), "crates/acn-trace/src/lib.rs", "changed\n");
    assert_eq!(engine_hash_of(dir.path()), before);
}

/// Cites: CON-28
#[test]
fn the_record_carries_both_hashes_and_check_verifies_both() {
    let dir = frozen_fixture();
    assert!(xtask_at(dir.path(), &["env-hash", "--write"]).ok());
    let record: serde_json::Value =
        serde_json::from_str(&fs::read_to_string(dir.path().join("env-hash.json")).expect("read"))
            .expect("json");
    assert_eq!(
        record["engine_hash"].as_str(),
        Some(engine_hash_of(dir.path()).as_str())
    );
    assert_eq!(
        record["env_hash"].as_str(),
        Some(hash_of(dir.path()).as_str())
    );
    assert!(xtask_at(dir.path(), &["env-hash", "--check"]).ok());

    // A record whose engine_hash is not the hash of its own frozen-crate entries is
    // inconsistent, even though env_hash and the file list are right.
    let mut forged = record.clone();
    forged["engine_hash"] = serde_json::Value::String("0".repeat(64));
    fs::write(
        dir.path().join("env-hash.json"),
        serde_json::to_string_pretty(&forged).expect("json"),
    )
    .expect("write");
    let run = xtask_at(dir.path(), &["env-hash", "--check"]);
    assert!(!run.ok(), "{}", run.json);
    let error = run.json["error"].as_str().expect("error");
    assert!(error.contains("engine_hash"), "{error}");
    assert!(error.contains("inconsistent"), "{error}");
}

/// Cites: CON-28
#[test]
fn a_record_written_before_engine_hash_existed_fails_the_check_with_a_way_out() {
    let dir = frozen_fixture();
    assert!(xtask_at(dir.path(), &["env-hash", "--write"]).ok());
    let mut record: serde_json::Value =
        serde_json::from_str(&fs::read_to_string(dir.path().join("env-hash.json")).expect("read"))
            .expect("json");
    record
        .as_object_mut()
        .expect("object")
        .remove("engine_hash");
    fs::write(
        dir.path().join("env-hash.json"),
        serde_json::to_string_pretty(&record).expect("json"),
    )
    .expect("write");
    let run = xtask_at(dir.path(), &["env-hash", "--check"]);
    assert!(!run.ok(), "{}", run.json);
    let error = run.json["error"].as_str().expect("error");
    assert!(error.contains("engine_hash"), "{error}");
    assert!(error.contains("--write"), "{error}");
    // Printing still works on the old record: only the gate insists.
    let print = xtask_at(dir.path(), &["env-hash"]);
    assert!(print.ok(), "{}", print.json);
    assert!(print.json["recorded"].is_null());
    assert!(xtask_at(dir.path(), &["env-hash", "--write"]).ok());
    assert!(xtask_at(dir.path(), &["env-hash", "--check"]).ok());
}

/// Cites: CON-28
#[test]
fn self_host_the_recorded_engine_hash_matches_the_workspace() {
    let record: serde_json::Value =
        serde_json::from_str(&fs::read_to_string(repo_root().join("env-hash.json")).expect("read"))
            .expect("json");
    let run = xtask_at(&repo_root(), &["env-hash"]);
    assert!(run.ok(), "{}", run.json);
    let recorded = record["engine_hash"]
        .as_str()
        .expect("engine_hash is recorded");
    assert!(
        recorded.len() == 64 && recorded.chars().all(|c| c.is_ascii_hexdigit()),
        "{recorded}"
    );
    assert_eq!(Some(recorded), run.json["engine_hash"].as_str());
    let engine_files = record["files"]
        .as_array()
        .expect("files")
        .iter()
        .filter(|f| f["path"].as_str().expect("path").starts_with("crates/"))
        .count();
    assert!(
        engine_files >= 3,
        "the three frozen crate paths are recorded"
    );
}
