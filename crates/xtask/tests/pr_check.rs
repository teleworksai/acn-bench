//! Tests for `cargo xtask pr-check`: label rules for specs (CON-14) and the
//! frozen set (CON-7), and CODEOWNERS coverage of protected paths (LOOP-20).

#![allow(clippy::unwrap_used, clippy::expect_used, clippy::panic)] // CON-19: tests are exempt

mod common;

use std::fs;
use std::path::Path;

use common::{repo_root, strings, xtask_at};

const FULL_CODEOWNERS: &str = "\
# owners
/.github/workflows/              @owner
/.github/dependabot.yml          @owner
/.cargo/                         @owner
/Cargo.toml                      @owner
/Cargo.lock                      @owner
/clippy.toml                     @owner
/deny.toml                       @owner
/rust-toolchain.toml             @owner
/tools/ci.sh                     @owner
/crates/xtask/                   @owner
/lab/clippy.toml                 @owner
/hypotheses/                     @owner
/scenarios/measured/             @owner
/crates/acn-hyp/                 @owner
/crates/acn-attrib/src/core/     @owner
/crates/acn-trace/src/schema/    @owner
/specs/                          @owner
/env-hash.json                   @owner
/trace-scope.toml                @owner
/docs/gates/                     @owner
/.github/CODEOWNERS              @owner
";

fn root_with(codeowners: &str, m0_closed: bool) -> tempfile::TempDir {
    let dir = tempfile::tempdir().expect("tempdir");
    write(dir.path(), ".github/CODEOWNERS", codeowners);
    if m0_closed {
        write(dir.path(), "docs/gates/M0.md", "# M0\n");
    }
    dir
}

fn write(root: &Path, rel: &str, body: &str) {
    let p = root.join(rel);
    fs::create_dir_all(p.parent().expect("parent")).expect("mkdir");
    fs::write(p, body).expect("write");
}

fn rules(v: &serde_json::Value) -> Vec<String> {
    v["violations"]
        .as_array()
        .expect("violations")
        .iter()
        .map(|x| x["label"].as_str().expect("label").to_owned())
        .collect()
}

/// Cites: CON-14
#[test]
fn a_spec_edit_without_the_spec_change_label_fails() {
    let dir = root_with(FULL_CODEOWNERS, false);
    let changed = "specs/010-trace-schema.md,crates/acn-trace/src/lib.rs";
    let bad = xtask_at(
        dir.path(),
        &["pr-check", "--changed", changed, "--labels", "lab"],
    );
    assert!(!bad.ok(), "{}", bad.json);
    assert_eq!(rules(&bad.json), vec!["spec-change"]);
    assert_eq!(
        strings(&bad.json["violations"][0], "paths"),
        vec!["specs/010-trace-schema.md"]
    );

    let good = xtask_at(
        dir.path(),
        &["pr-check", "--changed", changed, "--labels", "spec-change"],
    );
    assert!(good.ok(), "{}", good.json);
}

/// Cites: CON-7
#[test]
fn a_frozen_set_edit_needs_env_change_once_m0_is_closed() {
    let changed = "hypotheses/p4.toml,env-hash.json,crates/acn-attrib/src/core/mod.rs,crates/acn-attrib/src/lib.rs";

    let before = root_with(FULL_CODEOWNERS, false);
    let advisory = xtask_at(
        before.path(),
        &["pr-check", "--changed", changed, "--labels", ""],
    );
    assert!(
        advisory.ok(),
        "pre-M0 the rule is advisory: {}",
        advisory.json
    );
    assert_eq!(
        advisory.json["advisories"]
            .as_array()
            .expect("advisories")
            .len(),
        1
    );

    let after = root_with(FULL_CODEOWNERS, true);
    let bad = xtask_at(
        after.path(),
        &["pr-check", "--changed", changed, "--labels", "spec-change"],
    );
    assert!(!bad.ok(), "{}", bad.json);
    assert_eq!(rules(&bad.json), vec!["env-change"]);
    let paths = strings(&bad.json["violations"][0], "paths");
    assert_eq!(
        paths.len(),
        3,
        "lib.rs outside core/ is not frozen: {paths:?}"
    );

    let good = xtask_at(
        after.path(),
        &["pr-check", "--changed", changed, "--labels", "env-change"],
    );
    assert!(good.ok(), "{}", good.json);
}

/// Cites: CON-7, CON-14
#[test]
fn ordinary_changes_need_no_label() {
    let dir = root_with(FULL_CODEOWNERS, true);
    let run = xtask_at(
        dir.path(),
        &[
            "pr-check",
            "--changed",
            "crates/acn-emu/src/lib.rs,docs/lab/x.md",
            "--labels",
            "",
        ],
    );
    assert!(run.ok(), "{}", run.json);
}

/// Cites: LOOP-20
#[test]
fn codeowners_must_cover_every_protected_path() {
    let partial = FULL_CODEOWNERS.replace("/specs/                          @owner\n", "");
    let dir = root_with(&partial, false);
    let run = xtask_at(dir.path(), &["pr-check"]);
    assert!(!run.ok(), "{}", run.json);
    assert_eq!(strings(&run.json, "codeowners_missing"), vec!["/specs/"]);

    // A pattern without an owner does not count.
    let ownerless = FULL_CODEOWNERS.replace("/specs/                          @owner", "/specs/");
    let dir = root_with(&ownerless, false);
    let run = xtask_at(dir.path(), &["pr-check"]);
    assert_eq!(strings(&run.json, "codeowners_missing"), vec!["/specs/"]);

    let none = tempfile::tempdir().expect("tempdir");
    let run = xtask_at(none.path(), &["pr-check"]);
    assert!(!run.ok(), "a missing CODEOWNERS file fails: {}", run.json);
}

/// Cites: LOOP-20
#[test]
fn self_host_codeowners_covers_the_protected_paths() {
    let run = xtask_at(&repo_root(), &["pr-check"]);
    assert!(run.ok(), "{}", run.json);
}

// ---- git-mode tests on a real temporary repository (review round 1: B1, B2, S3) ----

/// Git for the scratch repositories, cut off from the developer's environment
/// and configuration: an inherited `GIT_DIR` (set by hooks and `rebase --exec`)
/// would make these commands write into the real repository, and a global hook
/// or template could make them fail.
fn git_command(root: &Path) -> std::process::Command {
    let mut cmd = std::process::Command::new("git");
    for var in [
        "GIT_DIR",
        "GIT_WORK_TREE",
        "GIT_INDEX_FILE",
        "GIT_OBJECT_DIRECTORY",
        "GIT_ALTERNATE_OBJECT_DIRECTORIES",
        "GIT_COMMON_DIR",
        "GIT_NAMESPACE",
        "GIT_PREFIX",
    ] {
        cmd.env_remove(var);
    }
    cmd.env("GIT_CONFIG_GLOBAL", "/dev/null")
        .env("GIT_CONFIG_NOSYSTEM", "1")
        .arg("-C")
        .arg(root)
        .args([
            "-c",
            "user.name=t",
            "-c",
            "user.email=t@example.com",
            "-c",
            "commit.gpgsign=false",
            "-c",
            "core.hooksPath=/dev/null",
        ]);
    cmd
}

fn git(root: &Path, args: &[&str]) {
    let out = git_command(root).args(args).output().expect("git");
    assert!(
        out.status.success(),
        "git {args:?}: {}",
        String::from_utf8_lossy(&out.stderr)
    );
}

fn git_out(root: &Path, args: &[&str]) -> String {
    let out = git_command(root).args(args).output().expect("git");
    String::from_utf8_lossy(&out.stdout).trim().to_owned()
}

/// A repo whose `main` has a spec, a frozen hypothesis, CODEOWNERS and (optionally) a closed M0 gate,
/// with a `work` branch checked out on top.
fn git_repo(m0_closed: bool) -> tempfile::TempDir {
    let dir = root_with(FULL_CODEOWNERS, m0_closed);
    write(dir.path(), "specs/000-constitution.md", "spec\n");
    write(dir.path(), "hypotheses/p4.toml", "[poc]\nid = \"p4\"\n");
    write(dir.path(), "README.md", "readme\n");
    git(dir.path(), &["init", "-q", "-b", "main"]);
    git(dir.path(), &["add", "-A"]);
    git(dir.path(), &["commit", "-q", "-m", "base"]);
    git(dir.path(), &["switch", "-q", "-c", "work"]);
    dir
}

/// Cites: CON-14, CON-7
#[test]
fn moving_a_file_out_of_a_protected_directory_is_still_a_change_to_it() {
    let dir = git_repo(true);
    git(
        dir.path(),
        &["mv", "specs/000-constitution.md", "moved-spec.md"],
    );
    git(dir.path(), &["mv", "hypotheses/p4.toml", "moved-hyp.toml"]);
    git(dir.path(), &["commit", "-q", "-m", "move"]);
    let run = xtask_at(dir.path(), &["pr-check", "--base", "main", "--labels", ""]);
    assert!(!run.ok(), "{}", run.json);
    let mut labels = rules(&run.json);
    labels.sort();
    assert_eq!(labels, vec!["env-change", "spec-change"], "{}", run.json);
}

/// Cites: CON-14
#[test]
fn non_ascii_paths_are_not_hidden_by_git_quoting() {
    let dir = git_repo(false);
    write(dir.path(), "specs/020-café.md", "new spec\n");
    git(dir.path(), &["add", "-A"]);
    git(dir.path(), &["commit", "-q", "-m", "add"]);
    let run = xtask_at(dir.path(), &["pr-check", "--base", "main", "--labels", ""]);
    assert!(!run.ok(), "{}", run.json);
    assert_eq!(rules(&run.json), vec!["spec-change"]);
}

/// Cites: CON-7
#[test]
fn deleting_the_m0_gate_record_does_not_reopen_the_frozen_set() {
    let dir = git_repo(true);
    git(dir.path(), &["rm", "-q", "docs/gates/M0.md"]);
    write(
        dir.path(),
        "hypotheses/p4.toml",
        "[poc]\nid = \"p4\"\n# edited\n",
    );
    git(dir.path(), &["add", "-A"]);
    git(dir.path(), &["commit", "-q", "-m", "sneaky"]);
    let run = xtask_at(dir.path(), &["pr-check", "--base", "main", "--labels", ""]);
    assert!(
        !run.ok(),
        "M0 state is read from the base as well as the head: {}",
        run.json
    );
    assert_eq!(rules(&run.json), vec!["env-change"]);
    assert_eq!(run.json["m0_closed"], true);
}

/// Cites: CON-8
#[test]
fn a_base_that_looks_like_an_option_is_rejected() {
    let dir = git_repo(false);
    let run = xtask_at(
        dir.path(),
        &["pr-check", "--base=--output=injected", "--labels", ""],
    );
    assert!(!run.ok(), "{}", run.json);
    assert!(
        run.json["error"]
            .as_str()
            .expect("error")
            .contains("--base"),
        "{}",
        run.json
    );
    assert!(!dir.path().join("injected...HEAD").exists());
}

/// Cites: CON-14
#[test]
fn changed_paths_are_normalised_before_matching() {
    let dir = root_with(FULL_CODEOWNERS, false);
    for p in ["./specs/010.md", "docs/../specs/010.md", "Specs/010.md"] {
        let run = xtask_at(dir.path(), &["pr-check", "--changed", p, "--labels", ""]);
        assert!(
            !run.ok(),
            "`{p}` must count as a specs/ change: {}",
            run.json
        );
    }
    let run = xtask_at(
        dir.path(),
        &[
            "pr-check",
            "--changed",
            "specs-old/x.md,hypotheses2/y.toml",
            "--labels",
            "",
        ],
    );
    assert!(run.ok(), "{}", run.json);
}

/// Cites: LOOP-20
#[test]
fn codeowners_coverage_follows_last_match_wins_and_needs_a_real_owner() {
    for (what, extra) in [
        ("a later ownerless entry", "/specs/\n"),
        ("a later catch-all", "* @bot\n"),
        ("a later narrower pattern", "/specs/*.md @bot\n"),
        ("a later parent pattern", "/crates/ @bot\n"),
        ("a glob inside a path component", "/spec* @bot\n"),
        ("a single-character glob", "/docs/gat?s/ @bot\n"),
        (
            "a bare file name, which matches at any depth",
            "p4.toml @bot\n",
        ),
        (
            "a bare directory name, which matches at any depth",
            "core/ @bot\n",
        ),
        ("a bare at-sign is not an owner", "/specs/ @\n"),
    ] {
        let dir = root_with(&format!("{FULL_CODEOWNERS}{extra}"), false);
        let run = xtask_at(dir.path(), &["pr-check"]);
        assert!(!run.ok(), "{what} must break coverage: {}", run.json);
    }
    let commented =
        FULL_CODEOWNERS.replace("/specs/                          @owner", "/specs/ # TODO");
    let dir = root_with(&commented, false);
    let run = xtask_at(dir.path(), &["pr-check"]);
    assert_eq!(
        strings(&run.json, "codeowners_missing"),
        vec!["/specs/"],
        "a comment is not an owner"
    );

    // An earlier catch-all is fine: the exact entry comes last.
    let dir = root_with(&format!("* @everyone\n{FULL_CODEOWNERS}"), false);
    assert!(xtask_at(dir.path(), &["pr-check"]).ok());
}

/// Cites: LOOP-20
#[test]
fn the_gate_records_and_codeowners_itself_are_protected() {
    for (line, pattern) in [
        ("/docs/gates/                     @owner\n", "/docs/gates/"),
        (
            "/.github/CODEOWNERS              @owner\n",
            "/.github/CODEOWNERS",
        ),
    ] {
        let dir = root_with(&FULL_CODEOWNERS.replace(line, ""), false);
        let run = xtask_at(dir.path(), &["pr-check"]);
        assert_eq!(strings(&run.json, "codeowners_missing"), vec![pattern]);
    }
}

/// Cites: LOOP-20, CON-9
#[test]
fn every_enforcement_point_needs_an_owner() {
    assert!(
        xtask::pr_check::ENFORCEMENT_POINTS.len() >= 11,
        "the list only grows"
    );
    for point in xtask::pr_check::ENFORCEMENT_POINTS {
        let line = FULL_CODEOWNERS
            .lines()
            .find(|l| l.split_whitespace().next() == Some(point))
            .unwrap_or_else(|| panic!("fixture lacks {point}"));
        let dir = root_with(&FULL_CODEOWNERS.replace(&format!("{line}\n"), ""), false);
        let run = xtask_at(dir.path(), &["pr-check"]);
        assert!(!run.ok(), "{point}: {}", run.json);
        assert_eq!(strings(&run.json, "codeowners_missing"), vec![*point]);
    }
}

// ---- pre-landing review of PR 2: every case below was a reproduced bypass or an unpinned branch ----

fn commit_all(root: &Path, msg: &str) {
    git(root, &["add", "-A"]);
    git(root, &["commit", "-q", "-m", msg]);
}

/// Cites: CON-8
#[test]
fn base_mode_reports_the_commits_and_the_changed_paths() {
    let dir = git_repo(false);
    let none = xtask_at(dir.path(), &["pr-check", "--base", "main", "--labels", ""]);
    assert!(none.ok(), "{}", none.json);
    assert_eq!(none.json["changed"], 0);

    write(dir.path(), "README.md", "edited\n");
    commit_all(dir.path(), "edit");
    let run = xtask_at(dir.path(), &["pr-check", "--base", "main", "--labels", ""]);
    assert!(run.ok(), "{}", run.json);
    assert_eq!(strings(&run.json, "changed_paths"), vec!["README.md"]);
    assert_eq!(
        run.json["base_sha"],
        git_out(dir.path(), &["rev-parse", "main"])
    );
    assert_eq!(
        run.json["head_sha"],
        git_out(dir.path(), &["rev-parse", "HEAD"])
    );
    assert_eq!(run.json["merge_base"], run.json["base_sha"]);
}

/// Cites: CON-7
#[test]
fn an_inherited_git_dir_cannot_redirect_the_check_into_another_repository() {
    let victim = git_repo(false);
    let before = (
        git_out(victim.path(), &["rev-parse", "HEAD"]),
        git_out(victim.path(), &["status", "--porcelain"]),
    );
    let dir = git_repo(true);
    write(
        dir.path(),
        "hypotheses/p4.toml",
        "[poc]\nid = \"p4\"\n# edited\n",
    );
    commit_all(dir.path(), "edit");
    let out = std::process::Command::new(env!("CARGO_BIN_EXE_xtask"))
        .env("GIT_DIR", victim.path().join(".git"))
        .env("GIT_WORK_TREE", victim.path())
        .env("GIT_INDEX_FILE", victim.path().join(".git/index"))
        .args([
            "--root",
            dir.path().to_str().expect("utf8"),
            "pr-check",
            "--base",
            "main",
            "--labels",
            "",
        ])
        .output()
        .expect("spawn");
    let json: serde_json::Value = serde_json::from_slice(&out.stdout).expect("one JSON object");
    assert_eq!(
        json["ok"], false,
        "the check must still see its own repository: {json}"
    );
    assert_eq!(strings(&json, "changed_paths"), vec!["hypotheses/p4.toml"]);
    let after = (
        git_out(victim.path(), &["rev-parse", "HEAD"]),
        git_out(victim.path(), &["status", "--porcelain"]),
    );
    assert_eq!(before, after, "the other repository must be untouched");
}

/// Cites: CON-7, CON-8
#[test]
fn an_unusable_base_fails_closed_whatever_the_gate_state() {
    for m0 in [false, true] {
        let dir = git_repo(m0);
        for base in ["nosuchref", "", "-x", "--output=injected"] {
            let arg = format!("--base={base}");
            let run = xtask_at(
                dir.path(),
                &["pr-check", &arg, "--labels", "env-change,spec-change"],
            );
            assert!(!run.ok(), "base `{base}`, m0={m0}: {}", run.json);
            assert!(run.json["error"].is_string(), "{}", run.json);
        }
        assert!(!dir.path().join("injected").exists());
    }
    // Not a git checkout at all.
    let plain = root_with(FULL_CODEOWNERS, false);
    let run = xtask_at(plain.path(), &["pr-check", "--base", "main"]);
    assert!(!run.ok(), "{}", run.json);
    // Unrelated histories: no merge base.
    let dir = git_repo(false);
    git(dir.path(), &["switch", "-q", "--orphan", "island"]);
    write(dir.path(), "x.md", "x\n");
    commit_all(dir.path(), "island");
    let run = xtask_at(dir.path(), &["pr-check", "--base", "main"]);
    assert!(!run.ok(), "{}", run.json);
    assert!(
        run.json["error"]
            .as_str()
            .expect("error")
            .contains("merge base")
    );
}

/// Cites: CON-7
#[test]
fn a_branch_forked_before_m0_still_sees_the_closed_gate() {
    let dir = git_repo(false);
    git(dir.path(), &["switch", "-q", "main"]);
    write(dir.path(), "docs/gates/M0.md", "# M0\n");
    commit_all(dir.path(), "close M0");
    git(dir.path(), &["switch", "-q", "work"]); // forked before the gate closed
    write(dir.path(), "hypotheses/late.toml", "[poc]\nid = \"late\"\n");
    commit_all(dir.path(), "frozen edit");
    let run = xtask_at(dir.path(), &["pr-check", "--base", "main", "--labels", ""]);
    assert_eq!(run.json["m0_closed"], true, "{}", run.json);
    assert_eq!(rules(&run.json), vec!["env-change"], "{}", run.json);
}

/// Cites: CON-14
#[test]
fn a_tag_that_shadows_the_base_branch_is_refused() {
    let dir = git_repo(false);
    write(dir.path(), "specs/000-constitution.md", "edited\n");
    commit_all(dir.path(), "spec edit");
    git(
        dir.path(),
        &["update-ref", "refs/remotes/origin/main", "main"],
    );
    let honest = xtask_at(
        dir.path(),
        &["pr-check", "--base", "origin/main", "--labels", ""],
    );
    assert_eq!(rules(&honest.json), vec!["spec-change"], "{}", honest.json);

    git(dir.path(), &["tag", "origin/main", "HEAD"]); // git would now resolve the tag first
    let shadowed = xtask_at(
        dir.path(),
        &["pr-check", "--base", "origin/main", "--labels", ""],
    );
    assert!(!shadowed.ok(), "{}", shadowed.json);
    assert!(
        shadowed.json["error"]
            .as_str()
            .expect("error")
            .contains("ambiguous"),
        "{}",
        shadowed.json
    );
    let full = xtask_at(
        dir.path(),
        &[
            "pr-check",
            "--base",
            "refs/remotes/origin/main",
            "--labels",
            "",
        ],
    );
    assert_eq!(
        rules(&full.json),
        vec!["spec-change"],
        "a full ref name is unambiguous: {}",
        full.json
    );
}

/// Cites: CON-7
#[test]
fn changing_or_deleting_a_gate_record_is_a_frozen_set_change() {
    let dir = git_repo(true);
    git(dir.path(), &["rm", "-q", "docs/gates/M0.md"]);
    commit_all(dir.path(), "delete the gate record, nothing else");
    let run = xtask_at(dir.path(), &["pr-check", "--base", "main", "--labels", ""]);
    assert_eq!(
        rules(&run.json),
        vec!["env-change"],
        "step one of a two-step reopen: {}",
        run.json
    );

    // Adding the next gate record is what a gate PR does and needs no label.
    let dir = git_repo(true);
    write(dir.path(), "docs/gates/M1.md", "# M1\n");
    commit_all(dir.path(), "close M1");
    let run = xtask_at(dir.path(), &["pr-check", "--base", "main", "--labels", ""]);
    assert!(run.ok(), "{}", run.json);

    // List mode has no history: a listed M0 record counts as closed and as changed.
    let plain = root_with(FULL_CODEOWNERS, false);
    let run = xtask_at(
        plain.path(),
        &[
            "pr-check",
            "--changed",
            "docs/gates/M0.md,hypotheses/p4.toml",
            "--labels",
            "",
        ],
    );
    assert_eq!(rules(&run.json), vec!["env-change"], "{}", run.json);
}

/// Cites: CON-12
#[test]
fn dropping_an_entry_from_the_scope_file_needs_a_label() {
    let dir = root_with(FULL_CODEOWNERS, false);
    write(
        dir.path(),
        "trace-scope.toml",
        "[[implemented]]\nspec = \"000\"\nids = [\"CON-7\", \"CON-12\"]\nsections = [\"3\"]\n",
    );
    git(dir.path(), &["init", "-q", "-b", "main"]);
    commit_all(dir.path(), "base");
    git(dir.path(), &["switch", "-q", "-c", "work"]);
    write(
        dir.path(),
        "trace-scope.toml",
        "[[implemented]]\nspec = \"000\"\nids = [\"CON-7\", \"CON-8\"]\n",
    );
    commit_all(dir.path(), "quietly stop requiring CON-12 and section 3");
    let run = xtask_at(dir.path(), &["pr-check", "--base", "main", "--labels", ""]);
    assert!(!run.ok(), "{}", run.json);
    assert_eq!(run.json["violations"][0]["rule"], "CON-12");
    assert_eq!(
        strings(&run.json["violations"][0], "paths"),
        vec!["000: CON-12", "000: section 3"]
    );
    let labelled = xtask_at(
        dir.path(),
        &["pr-check", "--base", "main", "--labels", "spec-change"],
    );
    assert!(
        labelled.ok(),
        "adding CON-8 is free; removals pass once labelled: {}",
        labelled.json
    );

    git(dir.path(), &["rm", "-q", "trace-scope.toml"]);
    commit_all(dir.path(), "delete the scope file");
    let run = xtask_at(dir.path(), &["pr-check", "--base", "main", "--labels", ""]);
    assert_eq!(
        strings(&run.json["violations"][0], "paths").len(),
        3,
        "{}",
        run.json
    );
}

/// Cites: CON-7
#[test]
fn a_gitlink_in_the_frozen_set_is_seen_even_when_gitmodules_says_ignore() {
    let dir = git_repo(true);
    let sha = git_out(dir.path(), &["rev-parse", "HEAD"]);
    write(
        dir.path(),
        ".gitmodules",
        "[submodule \"v\"]\n\tpath = hypotheses/vendored\n\turl = ./nowhere\n\tignore = all\n",
    );
    git(dir.path(), &["add", ".gitmodules"]);
    git(
        dir.path(),
        &[
            "update-index",
            "--add",
            "--cacheinfo",
            &format!("160000,{sha},hypotheses/vendored"),
        ],
    );
    git(dir.path(), &["commit", "-q", "-m", "vendored"]);
    let run = xtask_at(dir.path(), &["pr-check", "--base", "main", "--labels", ""]);
    assert!(
        strings(&run.json, "changed_paths").contains(&"hypotheses/vendored".to_owned()),
        "{}",
        run.json
    );
    assert_eq!(rules(&run.json), vec!["env-change"], "{}", run.json);
}

/// Cites: CON-14, CON-7
#[test]
fn paths_that_alias_a_protected_directory_are_classified_with_it() {
    let dir = root_with(FULL_CODEOWNERS, true);
    for (path, label) in [
        ("\u{17f}pecs/000-constitution.md", "spec-change"), // long s: same directory on APFS
        ("specs", "spec-change"),                           // the directory entry itself
        ("specs/..\\outside.md", "spec-change"),            // a backslash is a file-name character
        ("Hypotheses/P4.toml", "env-change"),
        ("ENV-HASH.JSON", "env-change"),
        ("crates/acn-hyp", "env-change"),
    ] {
        let run = xtask_at(dir.path(), &["pr-check", "--changed", path, "--labels", ""]);
        assert_eq!(rules(&run.json), vec![label], "`{path}`: {}", run.json);
    }
    for bad in ["/specs/a.md", "../outside.md"] {
        let run = xtask_at(dir.path(), &["pr-check", "--changed", bad, "--labels", ""]);
        assert!(
            run.json["error"].is_string(),
            "`{bad}` must be refused: {}",
            run.json
        );
    }
    // Look-alikes are not protected, even with the gate closed.
    let run = xtask_at(
        dir.path(),
        &[
            "pr-check",
            "--changed",
            "specs-old/x.md,hypotheses2/y.toml,.,docs/..",
            "--labels",
            "",
        ],
    );
    assert!(run.ok(), "{}", run.json);
    assert_eq!(run.json["changed"], 2);
    assert!(
        strings(&run.json, "violations").is_empty() && strings(&run.json, "advisories").is_empty()
    );
}

/// Cites: CON-7
#[test]
fn a_root_below_the_repository_top_level_is_refused() {
    let dir = tempfile::tempdir().expect("tempdir");
    let ws = dir.path().join("ws");
    write(&ws, ".github/CODEOWNERS", FULL_CODEOWNERS);
    write(&ws, "specs/000-constitution.md", "spec\n");
    git(dir.path(), &["init", "-q", "-b", "main"]);
    commit_all(dir.path(), "base");
    let run = xtask_at(&ws, &["pr-check", "--base", "main", "--labels", ""]);
    assert!(!run.ok(), "{}", run.json);
    assert!(
        run.json["error"]
            .as_str()
            .expect("error")
            .contains("top level"),
        "{}",
        run.json
    );
}

/// Cites: CON-14
#[test]
fn labels_as_json_cannot_be_split_on_commas() {
    let dir = root_with(FULL_CODEOWNERS, false);
    let spoof = xtask_at(
        dir.path(),
        &[
            "pr-check",
            "--changed",
            "specs/a.md",
            "--labels-json",
            "[\"not-a,spec-change\"]",
        ],
    );
    assert_eq!(rules(&spoof.json), vec!["spec-change"], "{}", spoof.json);
    let real = xtask_at(
        dir.path(),
        &[
            "pr-check",
            "--changed",
            "specs/a.md",
            "--labels-json",
            "[\"spec-change\"]",
        ],
    );
    assert!(real.ok(), "{}", real.json);
    let bad = xtask_at(
        dir.path(),
        &[
            "pr-check",
            "--changed",
            "specs/a.md",
            "--labels-json",
            "spec-change",
        ],
    );
    assert!(bad.json["error"].is_string(), "{}", bad.json);
}

/// Cites: LOOP-20
#[test]
fn codeowners_lines_github_would_not_honour_do_not_count() {
    let exact = "/specs/                          @owner";
    for (what, replacement) in [
        ("a doubled leading slash", "//specs/ @owner"),
        ("a non-owner token before the owner", "/specs/ TODO @owner"),
        (
            "a no-break space instead of a separator",
            "/specs/\u{a0}@owner",
        ),
        ("an invalid handle", "/specs/ @/"),
        ("an e-mail without a domain", "/specs/ a@b"),
    ] {
        let dir = root_with(&FULL_CODEOWNERS.replace(exact, replacement), false);
        let run = xtask_at(dir.path(), &["pr-check"]);
        assert_eq!(
            strings(&run.json, "codeowners_missing"),
            vec!["/specs/"],
            "{what}: {}",
            run.json
        );
    }
    let dir = root_with(
        &FULL_CODEOWNERS.replace(
            "/env-hash.json                   @owner",
            "/env-hash.json/ @owner",
        ),
        false,
    );
    let run = xtask_at(dir.path(), &["pr-check"]);
    assert_eq!(
        strings(&run.json, "codeowners_missing"),
        vec!["/env-hash.json"],
        "a directory-only pattern does not match the file: {}",
        run.json
    );

    for (what, extra, missing) in [
        (
            "a later literal file inside a protected directory",
            "/specs/000-constitution.md @bot\n",
            "/specs/",
        ),
        (
            "a later escaped spelling of the same path",
            "/spec\\s/\n",
            "/specs/",
        ),
        (
            "a later character-class glob",
            "/hypotheses/p[0-9].toml @bot\n",
            "/hypotheses/",
        ),
        (
            "a later entry without a leading slash",
            "docs/gates/ @bot\n",
            "/docs/gates/",
        ),
    ] {
        let dir = root_with(&format!("{FULL_CODEOWNERS}{extra}"), false);
        let run = xtask_at(dir.path(), &["pr-check"]);
        assert!(
            strings(&run.json, "codeowners_missing").contains(&missing.to_owned()),
            "{what}: {}",
            run.json
        );
    }

    let big = format!("# {}\n{FULL_CODEOWNERS}", "x".repeat(3 * 1024 * 1024 + 1));
    let dir = root_with(&big, false);
    let run = xtask_at(dir.path(), &["pr-check"]);
    assert!(
        run.json["error"]
            .as_str()
            .expect("error")
            .contains("ignores"),
        "GitHub ignores an oversized file"
    );
}
