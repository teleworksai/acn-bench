//! The committed digest list of the evidence files (SPEC 140 P16-20, ADR-57):
//! `cargo xtask evidence-digests` and the same check inside
//! `cargo xtask docs-inventory --check`.

#![allow(clippy::unwrap_used, clippy::expect_used, clippy::panic)] // CON-19: tests are exempt

mod common;

use std::fs;
use std::path::Path;

use common::{fixture, repo_root, strings, xtask_at};

const LOOP: &str = "5c0466be088be5faa4c3cef71d46ca74bc2cf1f942e688762be7a610595db7f5";
const L1: &str = "da68f08906d1a49e4ed7e7629c0852c53da943e1babf1f4d8135089b079f9b19";
const L2: &str = "28e657208493f8401ea356499bb5d63eb13c899b23c73475b38869a5003b9112";
const LIST: &str = "docs/evidence-digests.txt";

fn copy(from: &Path, to: &Path) {
    for e in walkdir::WalkDir::new(from) {
        let e = e.unwrap();
        let dest = to.join(e.path().strip_prefix(from).unwrap());
        if e.file_type().is_dir() {
            fs::create_dir_all(&dest).unwrap();
        } else {
            fs::copy(e.path(), &dest).unwrap();
        }
    }
}

/// The `ok` fixture with the loop's committed files, its list, and its pages
/// rendered, so every gate passes before a test changes anything.
fn root() -> tempfile::TempDir {
    let d = tempfile::tempdir().unwrap();
    copy(&fixture("ok"), d.path());
    copy(&fixture("evidence"), d.path());
    let run = xtask_at(d.path(), &["docs-inventory"]);
    assert!(run.ok(), "{}", run.json);
    for args in [
        &["docs-inventory", "--check"][..],
        &["evidence-digests", "--check"][..],
    ] {
        let run = xtask_at(d.path(), args);
        assert!(run.ok(), "{args:?}: {}", run.json);
    }
    d
}

fn l2_verdict() -> String {
    format!("docs/runs/verdicts/{L2}/verdict.json")
}

fn twin() -> String {
    format!("docs/runs/loop/{LOOP}/twin/{L2}/twin.json")
}

/// The digest part of a report: `diff` of `evidence-digests`, or
/// `evidence_digests.diff` of `docs-inventory`.
fn diff(json: &serde_json::Value) -> &serde_json::Value {
    json.get("evidence_digests")
        .map_or(&json["diff"], |p| &p["diff"])
}

/// Both gates refuse the tree, and name `rel` under `kind`.
fn both_fail(d: &Path, kind: &str, rel: &str) {
    for args in [
        &["evidence-digests", "--check"][..],
        &["docs-inventory", "--check"][..],
    ] {
        let run = xtask_at(d, args);
        assert!(!run.ok(), "{args:?} passed: {}", run.json);
        assert!(
            strings(diff(&run.json), kind).iter().any(|p| p == rel),
            "{args:?}: {rel} is not under `{kind}`: {}",
            run.json
        );
    }
}

/// Regenerating the pages is accepted and changes nothing about the list:
/// the tree still fails. Only `evidence-digests --write` repairs it, and that
/// changes the committed list.
fn only_a_new_list_repairs(d: &Path, kind: &str, rel: &str) {
    let before = fs::read(d.join(LIST)).unwrap();
    both_fail(d, kind, rel);
    let regen = xtask_at(d, &["docs-inventory"]);
    assert!(regen.ok(), "{}", regen.json);
    assert_eq!(fs::read(d.join(LIST)).unwrap(), before);
    both_fail(d, kind, rel);
    let write = xtask_at(d, &["evidence-digests", "--write"]);
    assert!(write.ok(), "{}", write.json);
    assert_ne!(fs::read(d.join(LIST)).unwrap(), before);
    for args in [
        &["evidence-digests", "--check"][..],
        &["docs-inventory", "--check"][..],
    ] {
        let run = xtask_at(d, args);
        assert!(run.ok(), "{args:?}: {}", run.json);
    }
}

/// Cites: P16-20
#[test]
fn the_list_names_every_committed_evidence_file_with_its_digest() {
    let d = root();
    let text = fs::read_to_string(d.path().join(LIST)).unwrap();
    let expected: Vec<String> = [
        format!("docs/runs/loop/{LOOP}/report.json"),
        twin(),
        l2_verdict(),
        format!("docs/runs/verdicts/{L1}/verdict.json"),
    ]
    .iter()
    .map(|rel| {
        let digest = blake3::hash(&fs::read(d.path().join(rel)).unwrap()).to_hex();
        format!("{digest}  {rel}\n")
    })
    .collect();
    // Sorted by path, one line per file, nothing else.
    assert_eq!(text, expected.concat());
    let run = xtask_at(d.path(), &["evidence-digests", "--check"]);
    assert_eq!(run.json["files"], 4, "{}", run.json);
    // Writing over a correct list changes nothing.
    assert!(xtask_at(d.path(), &["evidence-digests", "--write"]).ok());
    assert_eq!(fs::read_to_string(d.path().join(LIST)).unwrap(), text);
}

/// Cites: P16-20
#[test]
fn this_repositorys_evidence_files_are_as_listed() {
    let run = xtask_at(&repo_root(), &["evidence-digests", "--check"]);
    assert!(run.ok(), "{}", run.json);
    assert!(run.json["files"].as_u64().unwrap() >= 4, "{}", run.json);
}

/// Cites: P16-20
#[test]
fn a_verdict_field_the_page_does_not_show_cannot_be_edited_quietly() {
    let d = root();
    let p = d.path().join(l2_verdict());
    let text = fs::read_to_string(&p).unwrap();
    let from = "\"ignored_replicates\":[]";
    assert!(text.contains(from));
    fs::write(&p, text.replacen(from, "\"ignored_replicates\":[7]", 1)).unwrap();
    only_a_new_list_repairs(d.path(), "changed", &l2_verdict());
}

/// Cites: P16-20
#[test]
fn a_byte_that_changes_no_value_is_still_a_change() {
    let d = root();
    let p = d.path().join(twin());
    let mut bytes = fs::read(&p).unwrap();
    bytes.push(b'\n');
    fs::write(&p, bytes).unwrap();
    only_a_new_list_repairs(d.path(), "changed", &twin());
}

/// Cites: P16-20, LOOP-2
#[test]
fn a_twin_and_its_verdict_cannot_be_removed_together_quietly() {
    let d = root();
    fs::remove_dir_all(d.path().join(format!("docs/runs/loop/{LOOP}/twin"))).unwrap();
    fs::remove_dir_all(d.path().join(format!("docs/runs/verdicts/{L2}"))).unwrap();
    // The report does not name its twins, so the pages render without them.
    both_fail(d.path(), "absent", &l2_verdict());
    only_a_new_list_repairs(d.path(), "absent", &twin());
}

/// Cites: P16-20
#[test]
fn a_file_added_beside_the_committed_ones_is_unlisted() {
    let d = root();
    let rel = format!("docs/runs/loop/{LOOP}/note.txt");
    fs::write(d.path().join(&rel), "x\n").unwrap();
    only_a_new_list_repairs(d.path(), "unlisted", &rel);
}

/// Cites: P16-20
#[test]
fn evidence_files_without_a_list_fail_and_none_need_none() {
    let d = root();
    fs::remove_file(d.path().join(LIST)).unwrap();
    for args in [
        &["evidence-digests", "--check"][..],
        &["docs-inventory", "--check"][..],
    ] {
        let run = xtask_at(d.path(), args);
        assert!(!run.ok(), "{args:?}: {}", run.json);
        assert!(run.json.to_string().contains("is missing"), "{}", run.json);
    }
    // A tree with no evidence files needs no list (the small fixtures have none).
    let bare = tempfile::tempdir().unwrap();
    copy(&fixture("ok"), bare.path());
    assert!(xtask_at(bare.path(), &["docs-inventory"]).ok());
    for args in [
        &["evidence-digests", "--check"][..],
        &["docs-inventory", "--check"][..],
    ] {
        let run = xtask_at(bare.path(), args);
        assert!(run.ok(), "{args:?}: {}", run.json);
    }
    // And writing for such a tree creates none.
    assert!(xtask_at(bare.path(), &["evidence-digests", "--write"]).ok());
    assert!(!bare.path().join(LIST).exists());
}

/// Cites: P16-20
#[test]
fn a_list_that_is_not_exactly_what_would_be_written_fails() {
    let d = root();
    let good = fs::read_to_string(d.path().join(LIST)).unwrap();
    let lines: Vec<&str> = good.lines().collect();
    let reordered = format!("{}\n{}\n{}\n{}\n", lines[1], lines[0], lines[2], lines[3]);
    let doubled = format!("{good}{}\n", lines[0]);
    let upper = good.replacen(&lines[0][..64], &lines[0][..64].to_uppercase(), 1);
    for (bad, why) in [
        (reordered.as_str(), "order"),
        (doubled.as_str(), "a path listed twice"),
        (upper.as_str(), "an upper-case digest"),
        ("not a list\n", "no digest"),
        (good.trim_end(), "no final newline"),
        (&format!("{good}\n"), "a blank line"),
    ] {
        fs::write(d.path().join(LIST), bad).unwrap();
        for args in [
            &["evidence-digests", "--check"][..],
            &["docs-inventory", "--check"][..],
        ] {
            let run = xtask_at(d.path(), args);
            assert!(!run.ok(), "{why}, {args:?}: {}", run.json);
        }
    }
    // Write mode does not read the old list, so it repairs any of them.
    assert!(xtask_at(d.path(), &["evidence-digests", "--write"]).ok());
    assert_eq!(fs::read_to_string(d.path().join(LIST)).unwrap(), good);
}

/// Cites: P16-20
#[cfg(unix)]
#[test]
fn a_symlink_among_the_evidence_files_or_as_the_list_is_refused() {
    let d = root();
    let rel = l2_verdict();
    let outside = d.path().join("elsewhere.json");
    fs::rename(d.path().join(&rel), &outside).unwrap();
    std::os::unix::fs::symlink(&outside, d.path().join(&rel)).unwrap();
    // Same bytes through the link; still refused, in every mode.
    for args in [
        &["evidence-digests", "--check"][..],
        &["evidence-digests", "--write"][..],
        &["docs-inventory", "--check"][..],
    ] {
        let run = xtask_at(d.path(), args);
        assert!(!run.ok(), "{args:?}: {}", run.json);
        assert!(run.json.to_string().contains("symlink"), "{}", run.json);
    }
    fs::remove_file(d.path().join(&rel)).unwrap();
    fs::rename(&outside, d.path().join(&rel)).unwrap();
    assert!(xtask_at(d.path(), &["evidence-digests", "--check"]).ok());

    // The list itself: a link is neither read nor written through.
    let list = d.path().join(LIST);
    let target = d.path().join("list-elsewhere.txt");
    fs::rename(&list, &target).unwrap();
    std::os::unix::fs::symlink(&target, &list).unwrap();
    let kept = fs::read(&target).unwrap();
    for args in [
        &["evidence-digests", "--check"][..],
        &["evidence-digests", "--write"][..],
        &["docs-inventory", "--check"][..],
    ] {
        let run = xtask_at(d.path(), args);
        assert!(!run.ok(), "{args:?}: {}", run.json);
        assert!(run.json.to_string().contains("symlink"), "{}", run.json);
    }
    assert_eq!(fs::read(&target).unwrap(), kept);
}

/// Cites: P16-20, CON-8
#[test]
fn asking_for_neither_or_both_modes_is_refused() {
    let d = root();
    for args in [
        &["evidence-digests"][..],
        &["evidence-digests", "--check", "--write"][..],
    ] {
        let run = xtask_at(d.path(), args);
        assert!(!run.ok(), "{args:?}: {}", run.json);
    }
}
