//! Self-hosting tests for `cargo xtask trace-check` (CON-12) and its CLI
//! contract (CON-8). Fixtures live under `tests/fixtures/<case>/`, each a
//! miniature repo root with `specs/`, `tests/` and `trace-scope.toml`.

#![allow(clippy::unwrap_used, clippy::expect_used, clippy::panic)] // CON-19: tests are exempt

mod common;

use common::{fixture, repo_root, strings, xtask_at};

/// Cites: CON-12
#[test]
fn passes_when_every_implemented_must_is_cited() {
    let run = xtask_at(&fixture("ok"), &["trace-check"]);
    assert!(run.ok(), "{}", run.json);
    assert!(strings(&run.json, "missing").is_empty());
    assert!(strings(&run.json, "unknown_citations").is_empty());
    // Section scope: FIX-1 (MUST) is implemented, FIX-3 is out of scope, FIX-2 is a MAY.
    assert_eq!(run.json["implemented"], 2, "{}", run.json);
    assert_eq!(run.json["must_ids"], 2, "{}", run.json);
}

/// Cites: CON-12
#[test]
fn fails_on_an_uncited_must_in_an_implemented_section() {
    let run = xtask_at(&fixture("uncited"), &["trace-check"]);
    assert!(!run.ok(), "{}", run.json);
    let missing = run.json["missing"]
        .as_array()
        .expect("missing array")
        .iter()
        .map(|m| m["id"].as_str().expect("id").to_owned())
        .collect::<Vec<_>>();
    assert_eq!(missing, vec!["FIX-1".to_owned()], "{}", run.json);
}

/// Cites: CON-12
#[test]
fn fails_on_a_citation_of_a_nonexistent_id() {
    let run = xtask_at(&fixture("unknown_id"), &["trace-check"]);
    assert!(!run.ok(), "{}", run.json);
    let unknown = run.json["unknown_citations"]
        .as_array()
        .expect("unknown_citations array");
    assert_eq!(unknown.len(), 1, "{}", run.json);
    assert_eq!(unknown[0]["id"], "FIX-9");
    assert!(
        unknown[0]["file"]
            .as_str()
            .expect("file")
            .ends_with("tests/fixture.rs"),
        "{}",
        run.json
    );
    // Nothing is missing: FIX-1 is cited; the failure is the unknown ID alone.
    assert!(strings(&run.json, "missing").is_empty(), "{}", run.json);
}

/// Cites: CON-12
#[test]
fn fails_when_a_citation_is_not_attached_to_a_function() {
    let run = xtask_at(&fixture("detached"), &["trace-check"]);
    assert!(!run.ok(), "{}", run.json);
    let problems = run.json["problems"].as_array().expect("problems array");
    assert_eq!(problems.len(), 1, "{}", run.json);
    assert!(
        problems[0]["message"]
            .as_str()
            .expect("message")
            .contains("not attached to a function"),
        "{}",
        run.json
    );
}

/// Cites: CON-12
#[test]
fn ids_inside_code_fences_are_not_requirements() {
    let run = xtask_at(&fixture("ok"), &["trace-check"]);
    // FIX-1, FIX-2, FIX-3 only; FIX-9 sits inside a fence.
    assert_eq!(run.json["ids"], 3, "{}", run.json);
}

/// Cites: CON-12
#[test]
fn self_host_the_real_workspace_passes() {
    let run = xtask_at(&repo_root(), &["trace-check"]);
    assert!(run.ok(), "{}", run.json);
    assert!(run.json["specs"].as_u64().unwrap_or(0) >= 3, "{}", run.json);
}

/// Cites: CON-8
#[test]
fn trace_check_emits_one_json_object_and_exit_code_tracks_ok() {
    // `xtask_at` asserts the contract on both branches; exercise both.
    let good = xtask_at(&fixture("ok"), &["trace-check"]);
    assert_eq!(good.code, Some(0));
    let bad = xtask_at(&fixture("uncited"), &["trace-check"]);
    assert_eq!(bad.code, Some(1));
}

/// Cites: CON-8
#[test]
fn argument_errors_still_honour_the_json_contract() {
    let run = common::xtask(&["no-such-task"]);
    assert!(!run.ok());
    assert!(run.json["error"].is_string(), "{}", run.json);
    assert!(
        run.stderr.contains("no-such-task")
            || run.json["error"]
                .as_str()
                .is_some_and(|e| e.contains("no-such-task"))
    );
}

/// Cites: CON-12
#[test]
fn fails_when_a_citation_is_on_a_non_test_function() {
    let run = xtask_at(&fixture("not_a_test"), &["trace-check"]);
    assert!(!run.ok(), "{}", run.json);
    let problems = run.json["problems"].as_array().expect("problems array");
    assert_eq!(problems.len(), 2, "{}", run.json);
    assert!(
        problems.iter().all(|p| p["message"]
            .as_str()
            .expect("message")
            .contains("not a test function")),
        "{}",
        run.json
    );
    // The citation itself is not counted, so FIX-1 is also reported missing.
    assert_eq!(run.json["missing"][0]["id"], "FIX-1", "{}", run.json);
}

/// Cites: CON-12
#[test]
fn accepts_continuations_and_attribute_placements() {
    let run = xtask_at(&fixture("ok"), &["trace-check"]);
    assert!(run.ok(), "{}", run.json);
    assert!(strings(&run.json, "problems").is_empty(), "{}", run.json);
}

/// Cites: CON-12
#[test]
fn a_root_without_specs_is_an_error_not_a_pass() {
    let dir = tempfile::tempdir().expect("tempdir");
    let run = xtask_at(dir.path(), &["trace-check"]);
    assert!(!run.ok(), "{}", run.json);
    assert!(
        run.json["error"]
            .as_str()
            .expect("error")
            .contains("no specs/ directory"),
        "{}",
        run.json
    );
}

fn ids_of(v: &serde_json::Value, key: &str) -> Vec<String> {
    let mut ids: Vec<String> = v[key]
        .as_array()
        .unwrap_or_else(|| panic!("{key} array"))
        .iter()
        .map(|x| x["id"].as_str().expect("id").to_owned())
        .collect();
    ids.sort();
    ids
}

/// Cites: CON-12
#[test]
fn an_in_scope_id_needs_a_citation_even_without_an_rfc_keyword() {
    let run = xtask_at(&fixture("no_keyword"), &["trace-check"]);
    assert!(!run.ok(), "{}", run.json);
    assert_eq!(ids_of(&run.json, "missing"), vec!["FIX-1"], "{}", run.json);
}

/// Cites: CON-12
#[test]
fn dangling_id_references_in_docs_and_hypotheses_fail() {
    let run = xtask_at(&fixture("refs"), &["trace-check"]);
    assert!(!run.ok(), "{}", run.json);
    // FIX-21 docs/decisions/lab/note.md (only docs/lab itself is exempt), FIX-22 fenced block in
    // PLAN.md (fences outside specs/ hold real references), FIX-33 .github PR template,
    // FIX-44 and FIX-55 second endpoints of ranges in PLAN.md, FIX-66 hypothesis comment,
    // FIX-77 PLAN.md, FIX-88 ADR. docs/generated, docs/lab and lab/ are not scanned.
    assert_eq!(
        ids_of(&run.json, "dangling_references"),
        vec![
            "FIX-21", "FIX-22", "FIX-33", "FIX-44", "FIX-55", "FIX-66", "FIX-77", "FIX-88"
        ],
        "{}",
        run.json
    );
    let records = run.json["dangling_references"].as_array().expect("array");
    let at = |id: &str| {
        let r = records.iter().find(|r| r["id"] == id).expect(id);
        (
            r["file"].as_str().expect("file").to_owned(),
            r["line"].as_u64().expect("line"),
        )
    };
    assert_eq!(at("FIX-77"), ("PLAN.md".to_owned(), 6));
    assert_eq!(at("FIX-66"), ("hypotheses/p1.toml".to_owned(), 1));

    // `[poc].spec` is read as TOML. Reported: a missing spec under any quoting, a path outside
    // specs/ or with `..`, a non-numbered target, a value of the wrong type, invalid TOML, and a
    // file in a directory that only looks exempt. Not reported: p0 (spec exists), p1 (indexed,
    // unwritten), p5 (the `spec` key sits in another table).
    let mut specs: Vec<(String, String)> = run.json["dangling_spec_files"]
        .as_array()
        .expect("array")
        .iter()
        .map(|d| {
            (
                d["file"].as_str().expect("file").to_owned(),
                d["spec"].as_str().expect("spec").to_owned(),
            )
        })
        .collect();
    specs.sort();
    let expected = [
        (
            "hypotheses/generated/p8.toml",
            "specs/996-in-a-generated-dir.md",
        ),
        ("hypotheses/p2.toml", "specs/999-nowhere.md"),
        ("hypotheses/p3.toml", "specs/998-single-quoted-nowhere.md"),
        ("hypotheses/p4.toml", "specs/../hypotheses/p1.toml"),
        ("hypotheses/p6.toml", "<unparseable TOML>"),
        ("hypotheses/p7.toml", "<not a string>"),
        ("hypotheses/p9.toml", "specs/README.md"),
    ];
    let expected: Vec<(String, String)> = expected
        .iter()
        .map(|(f, s)| ((*f).to_owned(), (*s).to_owned()))
        .collect();
    assert_eq!(specs, expected, "{}", run.json);
    assert!(run.json["dangling_spec_files"][0]["reason"].is_string());
}

/// Cites: CON-12
#[test]
fn each_kind_of_dangling_reference_fails_the_gate_on_its_own() {
    // One root with only a dangling ID, one with only a dangling spec file: neither
    // condition may hide behind the other in the `ok` computation.
    let only_id = tempfile::tempdir().expect("tempdir");
    copy_dir(&fixture("ok"), only_id.path());
    std::fs::write(only_id.path().join("PLAN.md"), "Refers to FIX-404.\n").expect("write");
    let run = xtask_at(only_id.path(), &["trace-check"]);
    assert!(!run.ok(), "{}", run.json);
    assert_eq!(ids_of(&run.json, "dangling_references"), vec!["FIX-404"]);
    assert!(strings(&run.json, "dangling_spec_files").is_empty());

    let only_spec = tempfile::tempdir().expect("tempdir");
    copy_dir(&fixture("ok"), only_spec.path());
    std::fs::create_dir(only_spec.path().join("hypotheses")).expect("mkdir");
    std::fs::write(
        only_spec.path().join("hypotheses/p1.toml"),
        "[poc]\nspec = \"specs/404-nowhere.md\"\n",
    )
    .expect("write");
    let run = xtask_at(only_spec.path(), &["trace-check"]);
    assert!(!run.ok(), "{}", run.json);
    assert!(strings(&run.json, "dangling_references").is_empty());
    assert_eq!(
        run.json["dangling_spec_files"]
            .as_array()
            .expect("array")
            .len(),
        1
    );
}

/// Cites: CON-12
#[cfg(unix)]
#[test]
fn a_symlink_in_a_scanned_tree_is_refused() {
    let dir = tempfile::tempdir().expect("tempdir");
    copy_dir(&fixture("ok"), dir.path());
    std::fs::create_dir_all(dir.path().join("docs")).expect("mkdir");
    std::fs::write(dir.path().join("hidden.txt"), "FIX-404\n").expect("write");
    std::os::unix::fs::symlink(
        dir.path().join("hidden.txt"),
        dir.path().join("docs/design.md"),
    )
    .expect("symlink");
    let run = xtask_at(dir.path(), &["trace-check"]);
    assert!(!run.ok(), "{}", run.json);
    assert!(
        run.json["error"]
            .as_str()
            .expect("error")
            .contains("symlink"),
        "{}",
        run.json
    );
}

/// Cites: CON-12
#[test]
fn references_to_indexed_but_unwritten_specs_are_forward_not_dangling() {
    let run = xtask_at(&fixture("refs"), &["trace-check"]);
    let forward = strings(&run.json, "forward_references");
    assert!(forward.contains(&"LTR-4".to_owned()), "{}", run.json);
    assert!(
        forward.contains(&"specs/910-later.md".to_owned()),
        "{}",
        run.json
    );
    for not_an_id in ["UTF-8", "ADR-3", "H-1", "SHA-256"] {
        assert!(!forward.contains(&not_an_id.to_owned()), "{}", run.json);
    }
}

/// Cites: CON-12
#[test]
fn files_the_compiler_never_sees_cannot_satisfy_a_requirement() {
    // The orphan fixture cites FIX-1 three times: from a src file no `mod` line names,
    // from a file in a tests/ subdirectory, and from the virtual workspace root's tests/.
    // None of them is ever compiled. Only FIX-2, in a declared module, counts.
    let run = xtask_at(&fixture("orphan"), &["trace-check"]);
    assert!(!run.ok(), "{}", run.json);
    assert_eq!(run.json["cited_ids"], 1, "{}", run.json);
    assert_eq!(run.json["missing"][0]["id"], "FIX-1", "{}", run.json);
}

fn copy_dir(from: &std::path::Path, to: &std::path::Path) {
    for entry in walkdir::WalkDir::new(from) {
        let entry = entry.expect("walk");
        let dest = to.join(entry.path().strip_prefix(from).expect("prefix"));
        if entry.file_type().is_dir() {
            std::fs::create_dir_all(&dest).expect("mkdir");
        } else {
            std::fs::copy(entry.path(), &dest).expect("copy");
        }
    }
}

/// Cites: CON-12
#[test]
fn losing_the_scope_file_fails_closed() {
    let dir = tempfile::tempdir().expect("tempdir");
    copy_dir(&fixture("ok"), dir.path());
    std::fs::remove_file(dir.path().join("trace-scope.toml")).expect("rm");
    let run = xtask_at(dir.path(), &["trace-check"]);
    assert!(!run.ok(), "{}", run.json);
    assert!(
        run.json["error"]
            .as_str()
            .expect("error")
            .contains("trace-scope.toml"),
        "{}",
        run.json
    );
}

/// Cites: CON-12
#[test]
fn scope_file_mistakes_fail_the_check() {
    let dir = tempfile::tempdir().expect("tempdir");
    copy_dir(&fixture("ok"), dir.path());
    std::fs::write(
        dir.path().join("trace-scope.toml"),
        "[[implemented]]\nspec = \"900\"\nids = [\"FIX-1\", \"FIX-42\"]\nsections = [\"7\"]\n\n[[implemented]]\nspec = \"000\"\nids = [\"FIX-2\"]\n",
    )
    .expect("write");
    let run = xtask_at(dir.path(), &["trace-check"]);
    assert!(!run.ok(), "{}", run.json);
    // unknown ID, empty section, ID filed under the wrong spec
    assert_eq!(strings(&run.json, "scope_errors").len(), 3, "{}", run.json);

    std::fs::write(
        dir.path().join("trace-scope.toml"),
        "[[implemented]]\nspec = \"900\"\nidz = []\n",
    )
    .expect("write");
    let run = xtask_at(dir.path(), &["trace-check"]);
    assert!(!run.ok(), "an unknown key is rejected: {}", run.json);
}

/// Cites: CON-8
#[test]
fn help_version_no_arguments_and_conflicting_flags_keep_the_contract() {
    for args in [&["--help"][..], &["trace-check", "--help"][..]] {
        let run = common::xtask(args);
        assert!(run.ok(), "{args:?}: {}", run.json);
        assert!(
            run.stderr.contains("Usage"),
            "help goes to stderr: {}",
            run.stderr
        );
    }
    for args in [&[][..], &["env-hash", "--check", "--write"][..]] {
        let run = common::xtask(args);
        assert!(!run.ok(), "{args:?}: {}", run.json);
        assert!(run.json["error"].is_string());
    }
}
