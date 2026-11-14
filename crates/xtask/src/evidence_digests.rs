//! `cargo xtask evidence-digests` (SPEC 140 P16-20, ADR-57): a committed list
//! of the BLAKE3 of every file under `docs/runs/loop/` and
//! `docs/runs/verdicts/`.
//!
//! The evidence pages check what a committed file says against what it names
//! (`crate::evidence`), but a report does not name its twins, and a verdict
//! has fields that no id covers. So a twin removed with its verdict, or such a
//! field edited, renders a clean page. The list closes that: any byte added,
//! removed or changed under those directories fails `--check` until the list
//! changes too, and a changed list is a line in the diff that a reviewer
//! reads. `docs-inventory --check` runs the same check, so the gate chain
//! needs no new step; `docs-inventory` in write mode never touches the list.
//!
//! The walk fails closed: every entry is a directory or a regular file that
//! gets hashed, or it is an error. A symlink is never followed.

use std::collections::BTreeMap;
use std::fmt::Write as _;
use std::path::Path;

use serde::Serialize;

use crate::workspace::rel_strict;
use crate::{Error, Result};

/// The committed list. It is outside `docs/runs/`, whose files the gate
/// records' check reads as run records (ADR-57).
pub const LIST_FILE: &str = "docs/evidence-digests.txt";
/// The directories whose every file is listed (P16-20).
pub const COVERED: [&str; 2] = ["docs/runs/loop", "docs/runs/verdicts"];

const WRITE_HINT: &str = "if the change is intended, run `cargo xtask evidence-digests --write` and commit the list with it; a file that should not be committed is removed, not listed (P16-20)";

/// How the tree differs from the list, by repository-relative path.
#[derive(Debug, Default, PartialEq, Eq, Serialize)]
pub struct Diff {
    /// In the tree, not in the list.
    pub unlisted: Vec<String>,
    /// In the list, not in the tree.
    pub absent: Vec<String>,
    /// In both, with other bytes.
    pub changed: Vec<String>,
}

/// Why the check fails.
#[derive(Debug, Serialize)]
pub struct Problem {
    pub error: String,
    pub diff: Diff,
}

/// The JSON object `evidence-digests` prints (CON-8).
#[derive(Debug, Serialize)]
pub struct Report {
    pub ok: bool,
    pub mode: &'static str,
    pub list_file: &'static str,
    /// How many evidence files the tree holds.
    pub files: usize,
    #[serde(skip_serializing_if = "Option::is_none")]
    pub diff: Option<Diff>,
    #[serde(skip_serializing_if = "Option::is_none")]
    pub error: Option<String>,
}

/// What to do.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum Mode {
    Check,
    Write,
}

fn invalid<T>(m: impl Into<String>) -> Result<T> {
    Err(Error::Invalid(m.into()))
}

/// Refuse `rel` when it, or any directory on the way to it from `root`, is a
/// symlink: nothing is read or written through a link, wherever it sits. A
/// component that does not exist ends the check; there is nothing below it.
fn refuse_links(root: &Path, rel: &str) -> Result<()> {
    let mut path = root.to_path_buf();
    let mut seen = String::new();
    for part in rel.split('/') {
        path.push(part);
        if !seen.is_empty() {
            seen.push('/');
        }
        seen.push_str(part);
        match std::fs::symlink_metadata(&path) {
            Ok(m) if m.file_type().is_symlink() => {
                return invalid(format!(
                    "{seen} is a symlink; the evidence files and their list are regular files in regular directories of the repository (P16-20)"
                ));
            }
            Ok(_) => {}
            Err(_) => break,
        }
    }
    Ok(())
}

/// P16-20 names the committed files: a loop's `report.json`, each
/// `twin/<verdict_id>/twin.json` under it, and each verdict's `verdict.json`.
fn is_evidence(rel: &str) -> bool {
    let parts: Vec<&str> = rel.split('/').collect();
    matches!(
        parts.as_slice(),
        ["docs", "runs", "loop", _, "report.json"]
            | ["docs", "runs", "loop", _, "twin", _, "twin.json"]
            | ["docs", "runs", "verdicts", _, "verdict.json"]
    )
}

/// Path → lower-case hex BLAKE3 of every regular file under [`COVERED`].
pub fn compute(root: &Path) -> Result<BTreeMap<String, String>> {
    let mut files = BTreeMap::new();
    for covered in COVERED {
        let dir = root.join(covered);
        refuse_links(root, covered)?;
        if !dir.is_dir() {
            continue;
        }
        for entry in walkdir::WalkDir::new(&dir).follow_links(false) {
            let entry = entry?;
            let kind = entry.file_type();
            if kind.is_dir() {
                continue;
            }
            let rel = rel_strict(root, entry.path())?;
            if kind.is_symlink() {
                return invalid(format!(
                    "{rel} is a symlink; a committed evidence file is a regular file (P16-20)"
                ));
            }
            if !kind.is_file() {
                return invalid(format!("{rel} is not a regular file"));
            }
            if !is_evidence(&rel) {
                return invalid(format!(
                    "{rel} is not a file P16-20 names (`loop/<id>/report.json`, `loop/<id>/twin/<id>/twin.json`, `verdicts/<id>/verdict.json`): remove it; only those are committed under docs/runs/loop/ and docs/runs/verdicts/"
                ));
            }
            if rel.chars().any(char::is_control) {
                return invalid(format!(
                    "{rel:?} has a control character in its name and cannot be listed"
                ));
            }
            let bytes = std::fs::read(entry.path()).map_err(|e| Error::io(entry.path(), e))?;
            files.insert(rel, blake3::hash(&bytes).to_hex().to_string());
        }
    }
    Ok(files)
}

/// The list's text: one `<blake3>  <path>` line per file, sorted by path.
pub fn render(files: &BTreeMap<String, String>) -> String {
    let mut s = String::new();
    for (path, digest) in files {
        let _ = writeln!(s, "{digest}  {path}");
    }
    s
}

/// Read a list back. Only what [`render`] could have written line by line is
/// accepted; order and duplicates are the caller's comparison to make.
fn parse(text: &str) -> Result<Vec<(String, String)>> {
    let mut out = Vec::new();
    for (n, line) in text.lines().enumerate() {
        let entry = line.split_once("  ").filter(|(digest, path)| {
            digest.len() == 64
                && digest
                    .bytes()
                    .all(|b| b.is_ascii_digit() || (b'a'..=b'f').contains(&b))
                && !path.is_empty()
        });
        let Some((digest, path)) = entry else {
            return invalid(format!(
                "{LIST_FILE} line {}: expected `<64 lower-case hex>  <path>`; regenerate it with `cargo xtask evidence-digests --write`",
                n + 1
            ));
        };
        out.push((path.to_owned(), digest.to_owned()));
    }
    Ok(out)
}

fn diff(computed: &BTreeMap<String, String>, listed: &[(String, String)]) -> Diff {
    let listed_map: BTreeMap<&str, &str> = listed
        .iter()
        .map(|(p, d)| (p.as_str(), d.as_str()))
        .collect();
    let mut d = Diff::default();
    for (path, digest) in computed {
        match listed_map.get(path.as_str()) {
            None => d.unlisted.push(path.clone()),
            Some(l) if *l != digest => d.changed.push(path.clone()),
            Some(_) => {}
        }
    }
    for path in listed_map.keys() {
        if !computed.contains_key(*path) {
            d.absent.push((*path).to_owned());
        }
    }
    d
}

/// Compare the tree with the committed list. `Ok(None)` when they agree; a
/// tree with no evidence files needs no list.
pub fn check(root: &Path) -> Result<Option<Problem>> {
    let computed = compute(root)?;
    check_computed(root, &computed)
}

fn check_computed(root: &Path, computed: &BTreeMap<String, String>) -> Result<Option<Problem>> {
    let list = root.join(LIST_FILE);
    refuse_links(root, LIST_FILE)?;
    if !list.exists() {
        if computed.is_empty() {
            return Ok(None);
        }
        return Ok(Some(Problem {
            error: format!(
                "{LIST_FILE} is missing and the tree holds evidence files: run `cargo xtask evidence-digests --write`"
            ),
            diff: diff(computed, &[]),
        }));
    }
    let text = crate::workspace::read(&list)?;
    if text == render(computed) {
        return Ok(None);
    }
    let diff = diff(computed, &parse(&text)?);
    let error = if diff == Diff::default() {
        format!(
            "{LIST_FILE} names the right files and digests but is not written as `cargo xtask evidence-digests --write` writes it (one line per file, sorted by path)"
        )
    } else {
        format!("the evidence files under docs/runs/ differ from {LIST_FILE}: {WRITE_HINT}")
    };
    Ok(Some(Problem { error, diff }))
}

/// Run in `mode`.
pub fn run(root: &Path, mode: Mode) -> Result<Report> {
    let computed = compute(root)?;
    let mut report = Report {
        ok: true,
        mode: match mode {
            Mode::Check => "check",
            Mode::Write => "write",
        },
        list_file: LIST_FILE,
        files: computed.len(),
        diff: None,
        error: None,
    };
    match mode {
        Mode::Check => {
            if let Some(p) = check_computed(root, &computed)? {
                tracing::error!("{}", p.error);
                report.ok = false;
                report.error = Some(p.error);
                report.diff = Some(p.diff);
            }
        }
        Mode::Write => {
            // Write mode repairs a corrupt list, so it does not parse the old one.
            let list = root.join(LIST_FILE);
            refuse_links(root, LIST_FILE)?;
            if !computed.is_empty() || list.exists() {
                crate::workspace::write(&list, &render(&computed))?;
            }
        }
    }
    Ok(report)
}

#[cfg(test)]
mod tests {
    use super::*;

    /// Cites: P16-20
    #[test]
    fn a_line_is_a_lower_case_digest_two_spaces_and_a_path() {
        let d = "a".repeat(64);
        assert_eq!(
            parse(&format!("{d}  docs/runs/loop/x y/report.json\n")).ok(),
            Some(vec![(
                "docs/runs/loop/x y/report.json".to_owned(),
                d.clone()
            )])
        );
        for bad in [
            format!("{d} docs/x\n"),
            format!("{}  docs/x\n", &d[..63]),
            format!("{}  docs/x\n", d.to_uppercase()),
            format!("{d}  \n"),
            "\n".to_owned(),
        ] {
            assert!(parse(&bad).is_err(), "{bad:?}");
        }
    }

    /// Cites: P16-20
    #[test]
    fn only_the_files_the_spec_names_are_evidence() {
        for yes in [
            "docs/runs/loop/a/report.json",
            "docs/runs/loop/a/twin/b/twin.json",
            "docs/runs/verdicts/b/verdict.json",
        ] {
            assert!(is_evidence(yes), "{yes}");
        }
        for no in [
            "docs/runs/loop/a/.DS_Store",
            "docs/runs/loop/a/report.md",
            "docs/runs/loop/report.json",
            "docs/runs/loop/a/twin/twin.json",
            "docs/runs/loop/a/twin/b/verdict.json",
            "docs/runs/verdicts/b/c/verdict.json",
            "docs/runs/verdicts/verdict.json",
        ] {
            assert!(!is_evidence(no), "{no}");
        }
    }

    /// Cites: P16-20
    #[test]
    fn the_diff_names_what_was_added_removed_and_changed() {
        let computed: BTreeMap<String, String> = [("a", "1"), ("b", "2"), ("c", "3")]
            .into_iter()
            .map(|(p, d)| (p.to_owned(), d.to_owned()))
            .collect();
        let listed = vec![
            ("b".to_owned(), "2".to_owned()),
            ("c".to_owned(), "9".to_owned()),
            ("z".to_owned(), "4".to_owned()),
        ];
        let d = diff(&computed, &listed);
        assert_eq!(d.unlisted, ["a"]);
        assert_eq!(d.changed, ["c"]);
        assert_eq!(d.absent, ["z"]);
    }
}
