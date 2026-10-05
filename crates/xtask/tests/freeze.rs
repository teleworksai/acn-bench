//! HYP-26: the freeze PR's shape, as `cargo xtask pr-check` enforces it. Every
//! file a PR adds or changes under `hypotheses/` must be a `<id>.toml`
//! hypothesis that loads (its POC spec named, its status consistent), loads as
//! frozen (this PR records it), lints clean and, for `real-api`, carries its
//! pins; in `--base` mode, as the head commit has it. No label satisfies these.
//! (`crates/acn-hyp/tests/freeze.rs` checks the record half: `env-hash --check`.)

#![allow(clippy::unwrap_used, clippy::expect_used, clippy::panic)] // CON-19: tests are exempt

mod common;

use std::fs;
use std::path::Path;
use std::process::Command;

use common::xtask_at;
use xtask::pr_check::{self, Changes};

const HYP: &str = r#"[poc]
id = "t1"
title = "a test"
spec = "specs/100-x.md"

[hypothesis]
statement = "s"

[varies]
knob = { kind = "bool" }
mode = { kind = "enum", values = ["fast", "slow"] }

[measures]
primary = ["cached_token_ratio"]

[control]
description = "defaults"
config = { knob = false }

[design]
search = "grid"
replicates = 20
twin_required = false

[falsifier]
predicate = "max_over_knobs(abs(effect(cached_token_ratio))) < noise_floor(cached_token_ratio, control)"

[expected]
outcome = "pass"
"#;

fn write(root: &Path, rel: &str, body: &str) {
    let p = root.join(rel);
    fs::create_dir_all(p.parent().unwrap()).unwrap();
    fs::write(p, body).unwrap();
}

/// Write `env-hash.json` as `cargo xtask env-hash --write` would.
fn record(root: &Path) {
    let r = acn_trace::env::compute(root).unwrap();
    fs::write(
        root.join("env-hash.json"),
        serde_json::to_string(&r).unwrap(),
    )
    .unwrap();
}

/// A workspace root: every frozen-set directory, the spec catalogue, `files`,
/// and a record of the frozen set as it stands.
fn root(files: &[(&str, &str)]) -> tempfile::TempDir {
    let dir = tempfile::tempdir().unwrap();
    let r = dir.path();
    for d in acn_trace::env::FROZEN_SET {
        fs::create_dir_all(r.join(d)).unwrap();
    }
    write(r, "specs/README.md", "| 100 | 100-x.md | X | to write |\n");
    for (rel, text) in files {
        write(r, rel, text);
    }
    record(r);
    dir
}

/// The HYP-26 findings of a PR that changes `paths`, with no label.
fn findings(r: &Path, paths: &[&str]) -> Vec<(Vec<String>, String)> {
    let report = pr_check::run(
        r,
        Changes::List(paths.iter().map(|p| (*p).to_owned()).collect()),
        &[],
    )
    .unwrap();
    report
        .violations
        .iter()
        .filter(|v| v.rule == "HYP-26")
        .map(|v| {
            assert_eq!(v.label, None, "no label satisfies a content rule");
            (v.paths.clone(), v.message.clone())
        })
        .collect()
}

const FILE: &str = "hypotheses/t1.toml";

/// Cites: HYP-26, HYP-1, HYP-2, HYP-3, HYP-27
#[test]
fn pr_check_enforces_the_shape_of_a_freeze() {
    // A complete freeze, with no label at all (pre-M0): nothing to report.
    assert!(findings(root(&[(FILE, HYP)]).path(), &[FILE]).is_empty());
    // Moved into place but not recorded: still a candidate.
    let dir = root(&[]);
    write(dir.path(), FILE, HYP);
    let f = findings(dir.path(), &[FILE]);
    assert!(
        f[0].1.contains("loads as a candidate") && f[0].1.contains("env-hash --write"),
        "{f:?}"
    );
    // [poc].status left saying candidate: the load refuses the mismatch.
    let text = HYP.replace(
        "title = \"a test\"",
        "title = \"a test\"\nstatus = \"candidate\"",
    );
    let f = findings(root(&[(FILE, &text)]).path(), &[FILE]);
    assert!(
        f[0].1.contains("does not load") && f[0].1.contains("status"),
        "{f:?}"
    );
    assert!(
        !f[0].1.contains("/tmp") && !f[0].1.contains("/var/"),
        "repo-relative: {f:?}"
    );
    // No POC spec.
    let f = findings(
        root(&[(FILE, &HYP.replace("spec = \"specs/100-x.md\"\n", ""))]).path(),
        &[FILE],
    );
    assert!(
        f[0].1.contains("does not load") && f[0].1.contains("spec"),
        "{f:?}"
    );
    // A falsifier that can never fire, and a guard that disarms it: both reported.
    let bad = HYP
        .replace(
            "max_over_knobs(abs(effect(cached_token_ratio))) < noise_floor(cached_token_ratio, control)",
            "max_over_knobs(abs(effect(cached_token_ratio))) < 0",
        )
        .replace("[expected]", "inconclusive_if = \"replicates < 21\"\n\n[expected]");
    let f = findings(root(&[(FILE, &bad)]).path(), &[FILE]);
    assert!(
        f[0].1.contains("can never fire") && f[0].1.contains("guard"),
        "{f:?}"
    );
    // real-api without pins, then with them.
    let real = HYP.replace(
        "twin_required = false",
        "twin_required = false\nbackends = [\"real-api\"]",
    );
    let f = findings(root(&[(FILE, &real)]).path(), &[FILE]);
    assert!(f[0].1.contains("[design].pins"), "{f:?}");
    let pinned = real.replace(
        "backends = [\"real-api\"]",
        &format!(
            "backends = [\"real-api\"]\npins = {{ scenario = [\"{s}\"], workload = [\"{s}\"], models = {{}} }}",
            s = acn_trace::identity::Digest::of(b"x").to_hex()
        ),
    );
    assert!(findings(root(&[(FILE, &pinned)]).path(), &[FILE]).is_empty());
}

/// Cites: HYP-26, HYP-1
#[test]
fn every_file_under_hypotheses_is_checked_and_each_bad_one_reported() {
    let other = HYP.replace("id = \"t1\"", "id = \"t2\"");
    let sub = HYP.replace("id = \"t1\"", "id = \"t3\"");
    let dir = root(&[
        (FILE, HYP),
        (
            "hypotheses/t2.toml",
            &other.replace("spec = \"specs/100-x.md\"\n", ""),
        ),
        ("hypotheses/sub/t3.toml", &sub),
        ("hypotheses/README.md", "notes\n"),
        ("hypotheses/p5", &HYP.replace("id = \"t1\"", "id = \"p5\"")),
    ]);
    let paths = [
        "hypotheses/t1.toml",
        "hypotheses/t2.toml",
        "hypotheses/sub/t3.toml",
        "hypotheses/README.md",
        "hypotheses/p5",
    ];
    let f = findings(dir.path(), &paths);
    let flagged: Vec<&str> = f.iter().map(|(p, _)| p[0].as_str()).collect();
    assert_eq!(
        flagged,
        [
            "hypotheses/t2.toml",
            "hypotheses/README.md",
            "hypotheses/p5"
        ],
        "{f:?}"
    );
    assert!(f[1].1.contains("`<id>.toml`") && f[2].1.contains("`<id>.toml`"));
    // A removed file is the frozen-set change CON-7 covers, not a freeze.
    assert!(findings(root(&[]).path(), &[FILE]).is_empty());
    // Candidates under lab/ are not frozen and not checked (CON-23).
    assert!(
        findings(
            root(&[("lab/hypotheses/t1.toml", HYP)]).path(),
            &["lab/hypotheses/t1.toml"]
        )
        .is_empty()
    );
}

fn git(root: &Path, args: &[&str]) {
    let out = Command::new("git")
        .args([
            "-c",
            "user.name=t",
            "-c",
            "user.email=t@example.com",
            "-c",
            "commit.gpgsign=false",
        ])
        .args(args)
        .current_dir(root)
        .env_remove("GIT_DIR")
        .env_remove("GIT_WORK_TREE")
        .output()
        .unwrap();
    assert!(
        out.status.success(),
        "git {args:?}: {}",
        String::from_utf8_lossy(&out.stderr)
    );
}

fn hyp26(run: &common::Run) -> Vec<(String, String)> {
    run.json["violations"]
        .as_array()
        .unwrap()
        .iter()
        .filter(|v| v["rule"] == "HYP-26")
        .map(|v| {
            assert!(v["label"].is_null());
            (
                v["paths"][0].as_str().unwrap().to_owned(),
                v["message"].as_str().unwrap().to_owned(),
            )
        })
        .collect()
}

/// Cites: HYP-26
#[test]
fn in_base_mode_the_head_commit_is_judged() {
    let dir = root(&[(FILE, HYP)]);
    let r = dir.path();
    git(r, &["init", "-q", "-b", "main"]);
    git(r, &["add", "-A"]);
    git(r, &["commit", "-q", "-m", "base"]);
    git(r, &["switch", "-q", "-c", "work"]);
    // A recorded freeze, committed: no finding.
    write(
        r,
        "hypotheses/t2.toml",
        &HYP.replace("id = \"t1\"", "id = \"t2\""),
    );
    record(r);
    git(r, &["add", "-A"]);
    git(r, &["commit", "-q", "-m", "freeze t2"]);
    let run = xtask_at(r, &["pr-check", "--base", "main", "--labels", "env-change"]);
    assert_eq!(hyp26(&run), Vec::new(), "{}", run.json);
    // The same file edited in the working tree only: what is judged is not what
    // would merge.
    write(
        r,
        "hypotheses/t2.toml",
        &format!("{}# local\n", HYP.replace("id = \"t1\"", "id = \"t2\"")),
    );
    let f = hyp26(&xtask_at(
        r,
        &["pr-check", "--base", "main", "--labels", "env-change"],
    ));
    assert!(
        f.iter()
            .any(|(p, m)| p == "hypotheses/t2.toml" && m.contains("differs from the head commit")),
        "{f:?}"
    );
    git(r, &["checkout", "--", "hypotheses/t2.toml"]);
    // A rename: the new name is checked (its stem no longer matches its id).
    git(r, &["mv", "hypotheses/t2.toml", "hypotheses/t9.toml"]);
    record(r);
    git(r, &["add", "-A"]);
    git(r, &["commit", "-q", "-m", "rename"]);
    let f = hyp26(&xtask_at(
        r,
        &["pr-check", "--base", "main", "--labels", "env-change"],
    ));
    assert!(
        f.iter()
            .any(|(p, m)| p == "hypotheses/t9.toml" && m.contains("does not load")),
        "{f:?}"
    );
}
