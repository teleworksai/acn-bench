//! `cargo xtask pr-check` (ADR-6): the label rules for a pull request and the
//! CODEOWNERS coverage of protected paths.
//!
//! - CON-14: a change under `specs/` needs the `spec-change` label.
//! - CON-7: a change to the frozen set, to `env-hash.json`, or to an existing gate
//!   record needs the `env-change` label once the M0 gate is closed; before that
//!   it is reported as an advisory.
//! - CON-12: removing an entry from `trace-scope.toml` needs the `spec-change` label.
//! - LOOP-20: `.github/CODEOWNERS` must assign an owner to every protected
//!   path, under GitHub's last-match-wins rule.
//!
//! Everything here fails closed: an unresolvable or ambiguous base, a root that
//! is not the repository top level, a path that escapes the root, and any
//! CODEOWNERS line this parser does not fully understand all count against the PR.

use std::collections::BTreeSet;
use std::path::Path;
use std::process::Command;

use serde::Serialize;

use crate::env_hash::{FROZEN_SET, RECORD_FILE};
use crate::scope::{SCOPE_FILE, ScopeFile};
use crate::workspace::read;
use crate::{Error, Result};

const GATES_DIR: &str = "docs/gates";
const M0_RECORD: &str = "docs/gates/M0.md";
const SPECS_DIR: &str = "specs";
const CODEOWNERS: &str = ".github/CODEOWNERS";
const SPEC_CHANGE: &str = "spec-change";
const ENV_CHANGE: &str = "env-change";
/// GitHub ignores a CODEOWNERS file larger than 3 MB.
const MAX_CODEOWNERS_BYTES: u64 = 3 * 1024 * 1024;

/// A label rule that the PR violates (or, as an advisory, would violate after M0).
#[derive(Debug, Serialize)]
pub struct Finding {
    pub rule: &'static str,
    pub label: &'static str,
    pub paths: Vec<String>,
    pub message: String,
}

/// The JSON object `pr-check` prints (CON-8). It carries the resolved commits
/// and the changed paths so an empty diff cannot pass for a docs-only PR unseen.
#[derive(Debug, Serialize)]
pub struct Report {
    pub ok: bool,
    pub base: Option<String>,
    pub base_sha: Option<String>,
    pub merge_base: Option<String>,
    pub head_sha: Option<String>,
    pub changed: usize,
    pub changed_paths: Vec<String>,
    pub labels: Vec<String>,
    pub m0_closed: bool,
    pub violations: Vec<Finding>,
    pub advisories: Vec<Finding>,
    pub codeowners_missing: Vec<String>,
}

/// Where the changed-path list comes from.
#[derive(Debug, Clone)]
pub enum Changes {
    /// Only the static CODEOWNERS check.
    None,
    /// An explicit list of repo-relative paths (local use and tests; it has no
    /// history, so it treats any listed gate record as evidence that M0 is closed).
    List(Vec<String>),
    /// The commits between the merge base of `<base>` and HEAD.
    GitBase(String),
}

// ---------------------------------------------------------------- CODEOWNERS

/// Enforcement points: files that decide what the gates check or how they run.
/// A change to one of them can switch a gate off without touching a protected
/// path, so CODEOWNERS must name each (ADR-6, amendment 2). The manifest and the
/// lockfile are here because they choose the code the checker is built from.
pub const ENFORCEMENT_POINTS: &[&str] = &[
    "/.github/workflows/",
    "/.github/dependabot.yml",
    "/.cargo/",
    "/Cargo.toml",
    "/Cargo.lock",
    "/clippy.toml",
    "/deny.toml",
    "/rust-toolchain.toml",
    "/tools/ci.sh",
    "/crates/xtask/",
    "/lab/clippy.toml",
];

/// Paths CODEOWNERS must cover: the enforcement points, the frozen set, the
/// specs, the gate records, the two records the gates read, and CODEOWNERS
/// itself. These strings are the exact entries the file must contain.
pub fn protected_patterns() -> Vec<String> {
    let mut v: Vec<String> = ENFORCEMENT_POINTS.iter().map(|p| (*p).to_owned()).collect();
    v.extend(FROZEN_SET.iter().map(|p| format!("/{p}/")));
    v.push(format!("/{SPECS_DIR}/"));
    v.push(format!("/{RECORD_FILE}"));
    v.push(format!("/{SCOPE_FILE}"));
    v.push(format!("/{GATES_DIR}/"));
    v.push(format!("/{CODEOWNERS}"));
    v
}

/// `@user`, `@org/team`, or an e-mail address. Anything else is not an owner.
fn is_owner(token: &str) -> bool {
    let name = |s: &str| {
        !s.is_empty()
            && s.chars()
                .all(|c| c.is_ascii_alphanumeric() || "-_.".contains(c))
    };
    if let Some(handle) = token.strip_prefix('@') {
        return match handle.split_once('/') {
            Some((org, team)) => name(org) && name(team),
            None => name(handle),
        };
    }
    match token.split_once('@') {
        Some((local, domain)) => name(local) && domain.contains('.') && domain.split('.').all(name),
        None => false,
    }
}

/// One CODEOWNERS line as (pattern as written, owned). A line is owned only if
/// every token after the pattern is a valid owner and there is at least one:
/// GitHub skips a line with invalid syntax, so such a line protects nothing.
fn parse_line(line: &str) -> Option<(String, bool)> {
    // A comment starts at a `#` that begins the line or follows a space or tab.
    let mut code = line;
    let bytes = line.as_bytes();
    for (i, b) in bytes.iter().enumerate() {
        if *b == b'#' && (i == 0 || bytes[i - 1] == b' ' || bytes[i - 1] == b'\t') {
            code = &line[..i];
            break;
        }
    }
    // Only space and tab separate tokens; any other whitespace stays inside one.
    let mut tokens = code.split([' ', '\t']).filter(|t| !t.is_empty());
    let pattern = tokens.next()?.to_owned();
    let owners: Vec<&str> = tokens.collect();
    let owned = !owners.is_empty() && owners.iter().all(|o| is_owner(o));
    Some((pattern, owned))
}

/// Whether a CODEOWNERS pattern, as written, could decide ownership of files
/// under `protected` (a repo-relative path without slashes at either end).
/// Deliberately an over-approximation of gitignore matching:
/// - a negated (`!`) or unanchored pattern (`mod.rs`, `core/`, `*.md`) overlaps everything;
/// - backslash escapes are removed before comparing, so `/spec\s/` is `/specs/`;
/// - an anchored glob overlaps when its literal prefix and the protected path share a string prefix;
/// - an anchored literal overlaps when it is the path, a parent of it, or inside it.
fn overlaps(raw: &str, protected: &str) -> bool {
    if raw.starts_with('!') {
        return true;
    }
    let unescaped = raw.replace('\\', "");
    let anchored = unescaped.starts_with('/') || unescaped.trim_end_matches('/').contains('/');
    if !anchored {
        return true;
    }
    let pattern = unescaped.trim_start_matches('/').trim_end_matches('/');
    let is_glob = pattern.contains(['*', '?', '[']);
    let literal: String = pattern
        .chars()
        .take_while(|c| !matches!(c, '*' | '?' | '['))
        .collect();
    if is_glob {
        return protected.starts_with(&literal) || literal.starts_with(&format!("{protected}/"));
    }
    literal == protected
        || protected.starts_with(&format!("{literal}/"))
        || literal.starts_with(&format!("{protected}/"))
}

/// Protected patterns whose *last* overlapping CODEOWNERS entry is not the exact
/// protected entry with valid owners (GitHub applies the last matching pattern).
pub fn codeowners_missing(root: &Path) -> Result<Vec<String>> {
    let path = root.join(CODEOWNERS);
    let text = if path.is_file() {
        let size = std::fs::metadata(&path)
            .map_err(|e| Error::io(&path, e))?
            .len();
        if size > MAX_CODEOWNERS_BYTES {
            return Err(Error::Invalid(format!(
                "{CODEOWNERS} is {size} bytes; GitHub ignores a CODEOWNERS file over {MAX_CODEOWNERS_BYTES} bytes"
            )));
        }
        read(&path)?
    } else {
        String::new()
    };
    let entries: Vec<(String, bool)> = text.lines().filter_map(parse_line).collect();
    Ok(protected_patterns()
        .into_iter()
        .filter(|exact| {
            let protected = exact.trim_matches('/');
            let last = entries
                .iter()
                .rev()
                .find(|(raw, _)| overlaps(raw, protected));
            // Byte-for-byte the canonical entry: `//specs/` and `/env-hash.json/`
            // are different gitignore patterns and do not count.
            !matches!(last, Some((raw, true)) if raw == exact)
        })
        .collect())
}

// ----------------------------------------------------------------------- git

/// Run git for `root`, isolated from the caller's environment: a hook or
/// `rebase --exec` exports `GIT_DIR` and `GIT_INDEX_FILE`, which override `-C`.
fn git(root: &Path, args: &[&str]) -> Result<std::process::Output> {
    let mut cmd = Command::new("git");
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
    cmd.arg("-C").arg(root).args(args).output().map_err(|e| {
        Error::Invalid(format!(
            "cannot run `git` ({e}); `pr-check --base` needs git on PATH"
        ))
    })
}

fn first_line(bytes: &[u8]) -> String {
    String::from_utf8_lossy(bytes)
        .lines()
        .next()
        .unwrap_or("")
        .trim()
        .to_owned()
}

fn rev_parse(root: &Path, rev: &str) -> Result<Option<String>> {
    let spec = format!("{rev}^{{commit}}");
    let out = git(
        root,
        &[
            "rev-parse",
            "--verify",
            "--quiet",
            "--end-of-options",
            &spec,
        ],
    )?;
    Ok(out.status.success().then(|| first_line(&out.stdout)))
}

/// The three commits a PR check needs, resolved once and used as SHAs afterwards.
struct Commits {
    base_sha: String,
    merge_base: String,
    head_sha: String,
}

fn resolve(root: &Path, base: &str) -> Result<Commits> {
    if base.is_empty() || base.starts_with('-') {
        return Err(Error::Invalid(format!(
            "--base must be a git ref, got `{base}`"
        )));
    }
    // `git diff` prints paths relative to the repository top level, so the root must be it.
    let top = git(root, &["rev-parse", "--show-toplevel"])?;
    if !top.status.success() {
        return Err(Error::Invalid(format!(
            "{} is not a git checkout: {}",
            root.display(),
            first_line(&top.stderr)
        )));
    }
    let top_path = Path::new(&first_line(&top.stdout))
        .canonicalize()
        .map_err(|e| Error::io(root, e))?;
    let root_path = root.canonicalize().map_err(|e| Error::io(root, e))?;
    if top_path != root_path {
        return Err(Error::Invalid(format!(
            "--root {} is not the repository top level ({}); changed paths would not line up",
            root_path.display(),
            top_path.display()
        )));
    }
    // A short name that exists in more than one namespace is ambiguous, and git
    // prefers tags: a tag named `origin/main` would silently replace the base branch.
    let is_sha = base.len() >= 7 && base.chars().all(|c| c.is_ascii_hexdigit());
    if !base.starts_with("refs/") && !is_sha {
        let mut namespaces = Vec::new();
        for ns in ["refs/tags", "refs/heads", "refs/remotes"] {
            let full = format!("{ns}/{base}");
            if git(root, &["show-ref", "--verify", "--quiet", &full])?
                .status
                .success()
            {
                namespaces.push(full);
            }
        }
        if namespaces.len() > 1 {
            return Err(Error::Invalid(format!(
                "--base `{base}` is ambiguous ({}); pass a full ref name or a commit SHA",
                namespaces.join(", ")
            )));
        }
    }
    let base_sha = rev_parse(root, base)?.ok_or_else(|| {
        Error::Invalid(format!(
            "cannot resolve --base `{base}` to a commit (is that ref fetched? CI needs `fetch-depth: 0`)"
        ))
    })?;
    let head_sha = rev_parse(root, "HEAD")?
        .ok_or_else(|| Error::Invalid("cannot resolve HEAD to a commit".to_owned()))?;
    let mb = git(root, &["merge-base", &base_sha, &head_sha])?;
    if !mb.status.success() {
        return Err(Error::Invalid(format!(
            "no merge base between `{base}` and HEAD (unrelated histories, or a shallow clone)"
        )));
    }
    Ok(Commits {
        base_sha,
        merge_base: first_line(&mb.stdout),
        head_sha,
    })
}

/// Whether `path` exists in the tree of `sha`. A failing git command is an
/// error, never "absent": absence must not be inferred from a broken lookup.
fn exists_at(root: &Path, sha: &str, path: &str) -> Result<bool> {
    let out = git(root, &["ls-tree", "-z", "--name-only", sha, "--", path])?;
    if !out.status.success() {
        return Err(Error::Invalid(format!(
            "cannot read the tree of {sha}: {}",
            first_line(&out.stderr)
        )));
    }
    Ok(!out.stdout.is_empty())
}

fn show_at(root: &Path, sha: &str, path: &str) -> Result<Option<String>> {
    if !exists_at(root, sha, path)? {
        return Ok(None);
    }
    let out = git(root, &["show", &format!("{sha}:{path}")])?;
    if !out.status.success() {
        return Err(Error::Invalid(format!(
            "cannot read {path} at {sha}: {}",
            first_line(&out.stderr)
        )));
    }
    Ok(Some(String::from_utf8_lossy(&out.stdout).into_owned()))
}

/// Paths touched between two commits. `--no-renames` reports both sides of a
/// move, `-z` disables quoting, and `--ignore-submodules=none` overrides a
/// `.gitmodules` or config setting that would hide a changed gitlink.
fn changed_between(root: &Path, from: &str, to: &str) -> Result<Vec<String>> {
    let out = git(
        root,
        &[
            "diff",
            "--no-renames",
            "--ignore-submodules=none",
            "--name-only",
            "-z",
            from,
            to,
        ],
    )?;
    if !out.status.success() {
        return Err(Error::Invalid(format!(
            "git diff {from} {to} failed: {}",
            first_line(&out.stderr)
        )));
    }
    Ok(out
        .stdout
        .split(|b| *b == 0)
        .filter(|s| !s.is_empty())
        .map(|s| String::from_utf8_lossy(s).into_owned())
        .collect())
}

// --------------------------------------------------------------------- paths

/// Lexically normalise a path given on the command line: drop `./`, resolve
/// `..`. An absolute path, or one that climbs out of the root, is an error. A
/// backslash is an ordinary file-name character on the supported platforms and
/// is left alone.
fn normalise_path(path: &str) -> Result<String> {
    if path.starts_with('/') {
        return Err(Error::Invalid(format!(
            "--changed takes repo-relative paths, got `{path}`"
        )));
    }
    let mut parts: Vec<&str> = Vec::new();
    for c in path.split('/') {
        match c {
            "" | "." => {}
            ".." => {
                if parts.pop().is_none() {
                    return Err(Error::Invalid(format!(
                        "--changed path `{path}` climbs out of the root"
                    )));
                }
            }
            other => parts.push(other),
        }
    }
    Ok(parts.join("/"))
}

/// Full case folding, as a case-insensitive filesystem applies it: `Specs` and
/// `ſpecs` (U+017F) both name the `specs` directory on the primary platform (CON-1).
fn fold(s: &str) -> String {
    s.to_lowercase().to_uppercase().to_lowercase()
}

/// `path` is `dir` itself (a gitlink or symlink put in its place) or lies under it.
fn is_under(path: &str, dir: &str) -> bool {
    let (p, d) = (fold(path), fold(dir));
    p == d || p.strip_prefix(&d).is_some_and(|rest| rest.starts_with('/'))
}

fn is_frozen(path: &str) -> bool {
    fold(path) == fold(RECORD_FILE) || FROZEN_SET.iter().any(|f| is_under(path, f))
}

/// Entries of a scope file as comparable strings.
fn scope_entries(text: &str) -> Result<BTreeSet<String>> {
    let scope: ScopeFile = toml::from_str(text)
        .map_err(|e| Error::Invalid(format!("{SCOPE_FILE} does not parse: {e}")))?;
    let mut out = BTreeSet::new();
    for e in &scope.implemented {
        out.extend(e.ids.iter().map(|id| format!("{}: {id}", e.spec)));
        out.extend(
            e.sections
                .iter()
                .map(|s| format!("{}: section {s}", e.spec)),
        );
    }
    Ok(out)
}

// ----------------------------------------------------------------------- run

/// Run the check.
pub fn run(root: &Path, changes: Changes, labels: &[String]) -> Result<Report> {
    if !root.is_dir() {
        return Err(Error::Invalid(format!(
            "--root {} is not a directory",
            root.display()
        )));
    }
    let mut m0_closed = root.join(M0_RECORD).is_file();
    let mut commits: Option<(String, Commits)> = None;
    // Gate records that already existed: changing or deleting one is a frozen-set change.
    let mut existing_gate_records: Vec<String> = Vec::new();
    let mut scope_removed: Vec<String> = Vec::new();

    let changed: Vec<String> = match changes {
        Changes::None => Vec::new(),
        Changes::List(list) => {
            let mut v = Vec::new();
            for p in &list {
                let n = normalise_path(p)?;
                if !n.is_empty() {
                    v.push(n);
                }
            }
            // No history here: a listed gate record is taken as existing, which
            // also means deleting M0.md cannot reopen the frozen set.
            existing_gate_records = v
                .iter()
                .filter(|p| is_under(p, GATES_DIR))
                .cloned()
                .collect();
            m0_closed = m0_closed || v.iter().any(|p| fold(p) == fold(M0_RECORD));
            v
        }
        Changes::GitBase(base) => {
            let c = resolve(root, &base)?;
            // The gate counts as closed if the record is on the merge base, on the
            // tip of the base, or on the head: a branch forked before M0 must not
            // see an open gate, and a PR cannot reopen it by deleting the record.
            for sha in [&c.merge_base, &c.base_sha, &c.head_sha] {
                m0_closed = m0_closed || exists_at(root, sha, M0_RECORD)?;
            }
            let v = changed_between(root, &c.merge_base, &c.head_sha)?;
            for p in v.iter().filter(|p| is_under(p, GATES_DIR)) {
                if exists_at(root, &c.merge_base, p)? {
                    existing_gate_records.push(p.clone());
                }
            }
            if let Some(before) = show_at(root, &c.merge_base, SCOPE_FILE)? {
                let after = show_at(root, &c.head_sha, SCOPE_FILE)?.unwrap_or_default();
                let (before, after) = (scope_entries(&before)?, scope_entries(&after)?);
                scope_removed = before.difference(&after).cloned().collect();
            }
            commits = Some((base, c));
            v
        }
    };

    let has = |l: &str| labels.iter().any(|x| x == l);
    let mut violations = Vec::new();
    let mut advisories = Vec::new();

    let spec_paths: Vec<String> = changed
        .iter()
        .filter(|p| is_under(p, SPECS_DIR))
        .cloned()
        .collect();
    if !spec_paths.is_empty() && !has(SPEC_CHANGE) {
        violations.push(Finding {
            rule: "CON-14",
            label: SPEC_CHANGE,
            paths: spec_paths,
            message: format!(
                "{SPECS_DIR}/ changed: label the PR `{SPEC_CHANGE}` and state the rationale and the IDs added, changed or retired"
            ),
        });
    }

    if !scope_removed.is_empty() && !has(SPEC_CHANGE) {
        violations.push(Finding {
            rule: "CON-12",
            label: SPEC_CHANGE,
            paths: scope_removed,
            message: format!(
                "{SCOPE_FILE} no longer lists these entries, so their tests stop being required: label the PR `{SPEC_CHANGE}` and say why the requirement is no longer implemented"
            ),
        });
    }

    let mut frozen_paths: Vec<String> = changed.iter().filter(|p| is_frozen(p)).cloned().collect();
    frozen_paths.extend(existing_gate_records);
    if !frozen_paths.is_empty() && !has(ENV_CHANGE) {
        let (list, message) = if m0_closed {
            (
                &mut violations,
                format!(
                    "the frozen set or an existing gate record changed: label the PR `{ENV_CHANGE}`, include the updated `cargo xtask env-hash --write` output, and request an adversarial review"
                ),
            )
        } else {
            (
                &mut advisories,
                format!(
                    "the frozen set changed; after the M0 gate this will require the `{ENV_CHANGE}` label (CON-7)"
                ),
            )
        };
        list.push(Finding {
            rule: "CON-7",
            label: ENV_CHANGE,
            paths: frozen_paths,
            message,
        });
    }

    let missing = codeowners_missing(root)?;
    for v in &violations {
        tracing::error!(rule = v.rule, label = v.label, "{}", v.message);
    }
    for a in &advisories {
        tracing::warn!(rule = a.rule, label = a.label, "{}", a.message);
    }
    for m in &missing {
        tracing::error!(pattern = %m, "the last CODEOWNERS entry that can match this protected path is not the exact entry with valid owners (LOOP-20)");
    }
    let (base, base_sha, merge_base, head_sha) = match commits {
        Some((b, c)) => (
            Some(b),
            Some(c.base_sha),
            Some(c.merge_base),
            Some(c.head_sha),
        ),
        None => (None, None, None, None),
    };
    Ok(Report {
        ok: violations.is_empty() && missing.is_empty(),
        base,
        base_sha,
        merge_base,
        head_sha,
        changed: changed.len(),
        changed_paths: changed,
        labels: labels.to_vec(),
        m0_closed,
        violations,
        advisories,
        codeowners_missing: missing,
    })
}

#[cfg(test)]
mod tests {
    use super::*;

    /// Cites: CON-7
    #[test]
    fn frozen_paths_including_the_directory_itself_and_case_folding() {
        for p in [
            "hypotheses/p4.toml",
            "hypotheses", // a gitlink or symlink put in place of the directory
            "env-hash.json",
            "ENV-HASH.JSON",
            "Hypotheses/P4.toml",
            "Crates/ACN-Hyp/src/lib.rs",
            "crates/acn-attrib/src/core/mod.rs",
            "\u{17f}cenarios/measured/x.parquet", // long s folds to s
        ] {
            assert!(is_frozen(p), "{p} must be frozen");
        }
        for p in [
            "crates/acn-attrib/src/lib.rs",
            "hypotheses-old/x.toml",
            "hypotheses2/y.toml",
            "lab/hypotheses/p17.toml",
        ] {
            assert!(!is_frozen(p), "{p} must not be frozen");
        }
        assert!(is_under("\u{17f}pecs/000.md", SPECS_DIR));
        assert!(is_under("specs", SPECS_DIR));
        assert!(!is_under("specs-old/x.md", SPECS_DIR));
    }

    /// Cites: CON-14
    #[test]
    fn path_normalisation() {
        assert_eq!(
            normalise_path("./specs/a.md").ok().as_deref(),
            Some("specs/a.md")
        );
        assert_eq!(
            normalise_path("docs/../specs//a.md").ok().as_deref(),
            Some("specs/a.md")
        );
        assert_eq!(normalise_path("docs/..").ok().as_deref(), Some(""));
        // A backslash is a legal file-name character: `specs/..\x.md` is a file inside specs/.
        assert_eq!(
            normalise_path("specs/..\\x.md").ok().as_deref(),
            Some("specs/..\\x.md")
        );
        assert!(normalise_path("../../x").is_err());
        assert!(normalise_path("/specs/a.md").is_err());
    }

    /// Cites: LOOP-20
    #[test]
    fn overlap_is_an_over_approximation() {
        for (raw, protected) in [
            ("*", "specs"),
            ("/specs/*.md", "specs"),
            ("/specs/000-constitution.md", "specs"),
            ("/crates/acn-hyp/src/lib.rs", "crates/acn-hyp"),
            ("/crates/", "crates/acn-hyp"),
            ("/spec*", "specs"),
            ("/docs/gat?s/", "docs/gates"),
            ("/hypotheses/p[0-9].toml", "hypotheses"),
            ("/.git*/CODEOWNERS", ".github/CODEOWNERS"),
            ("mod.rs", "crates/acn-attrib/src/core"),
            ("core/", "crates/acn-attrib/src/core"),
            ("docs/gates/", "docs/gates"),
            ("/spec\\s/", "specs"),
            ("/\\specs/000-constitution.md", "specs"),
            ("!/specs/", "hypotheses"),
        ] {
            assert!(
                overlaps(raw, protected),
                "`{raw}` must overlap `{protected}`"
            );
        }
        for (raw, protected) in [
            ("/crates/acn-emu/", "crates/acn-hyp"),
            ("/specs-old/", "specs"),
            ("/spec-notes*", "specs"),
            ("docs/other/", "docs/gates"),
        ] {
            assert!(
                !overlaps(raw, protected),
                "`{raw}` must not overlap `{protected}`"
            );
        }
    }

    /// Cites: LOOP-20
    #[test]
    fn owners_and_lines() {
        for ok in ["@user", "@org/team", "@some-bot", "a@b.io"] {
            assert!(is_owner(ok), "{ok}");
        }
        for bad in ["@", "@/", "@@", "@a/", "#", "TODO", "a@b", "x@y.", "@a b"] {
            assert!(!is_owner(bad), "{bad}");
        }
        assert_eq!(
            parse_line("/specs/ @a @b # why"),
            Some(("/specs/".to_owned(), true))
        );
        assert_eq!(
            parse_line("/specs/ TODO @a"),
            Some(("/specs/".to_owned(), false))
        );
        assert_eq!(parse_line("/specs/"), Some(("/specs/".to_owned(), false)));
        // A no-break space is not a separator: the whole thing is one ownerless pattern.
        assert_eq!(
            parse_line("/specs/\u{a0}@a"),
            Some(("/specs/\u{a0}@a".to_owned(), false))
        );
        assert_eq!(parse_line("# comment"), None);
        assert_eq!(parse_line("   "), None);
    }

    /// Cites: CON-12
    #[test]
    fn scope_entries_are_comparable() {
        let s = scope_entries(
            "[[implemented]]\nspec = \"000\"\nids = [\"CON-7\"]\nsections = [\"3\"]\n",
        )
        .expect("parse");
        assert_eq!(
            s.into_iter().collect::<Vec<_>>(),
            ["000: CON-7", "000: section 3"]
        );
        assert!(scope_entries("[[implemented]]\nspec = 1\n").is_err());
    }
}
