//! Requirement-ID references outside Rust sources (ADR-3, amendment 2).
//!
//! Markdown at the root and under `docs/`, `specs/` and `.github/`, and
//! hypothesis files under `hypotheses/`, are scanned for `PREFIX-n` tokens. A
//! token whose prefix belongs to a written spec must name a defined ID; a token
//! whose prefix is only listed in `specs/README.md` for a spec that is not yet
//! written is a forward reference; any other token (`UTF-8`, `ADR-3`, `H-1`) is
//! not a requirement ID. Exactly two directories are exempt, `docs/generated/`
//! and `docs/lab/` (CON-23); `lab/` is never a scan root. Symlinks inside a
//! scanned tree are refused: their targets would escape the scan.

use std::collections::BTreeSet;
use std::path::{Path, PathBuf};

use serde::Serialize;
use walkdir::WalkDir;

use crate::specs::{Fences, Requirement, spec_number};
use crate::workspace::{read, rel};
use crate::{Error, Result};

const EXEMPT_DIRS: [&str; 2] = ["docs/generated", "docs/lab"];

/// One reference to a requirement ID in a non-Rust file.
#[derive(Debug, Clone, PartialEq, Eq, Serialize)]
pub struct Reference {
    pub id: String,
    pub file: String,
    pub line: usize,
}

/// A hypothesis file whose `[poc].spec` is unusable, with the reason.
#[derive(Debug, Clone, Serialize)]
pub struct DanglingSpecFile {
    pub spec: String,
    pub file: String,
    pub reason: String,
}

/// Result of the reference scan.
#[derive(Debug, Default, Serialize)]
pub struct RefScan {
    pub dangling: Vec<Reference>,
    pub dangling_spec_files: Vec<DanglingSpecFile>,
    /// Unique forward references: IDs of unwritten specs and unwritten spec files.
    pub forward: BTreeSet<String>,
    pub files_scanned: usize,
}

/// Every `PREFIX-n` token in `line`. A token is word-bounded; a trailing `(a)`
/// or letter is not part of it; a range `PREFIX-a..b` or `PREFIX-a–b` yields a
/// second token for its endpoint.
pub fn id_tokens(line: &str) -> Vec<String> {
    let bytes = line.as_bytes();
    let mut out = Vec::new();
    let mut i = 0;
    while i < bytes.len() {
        let boundary = i == 0 || !(bytes[i - 1].is_ascii_alphanumeric() || bytes[i - 1] == b'-');
        if !(boundary && bytes[i].is_ascii_uppercase()) {
            i += 1;
            continue;
        }
        let start = i;
        while i < bytes.len() && (bytes[i].is_ascii_uppercase() || bytes[i].is_ascii_digit()) {
            i += 1;
        }
        if i < bytes.len() && bytes[i] == b'-' {
            let digits_start = i + 1;
            let mut j = digits_start;
            while j < bytes.len() && bytes[j].is_ascii_digit() {
                j += 1;
            }
            if j > digits_start {
                out.push(line[start..j].to_owned());
                let rest = &line[j..];
                let after_sep = rest.strip_prefix("..").or_else(|| rest.strip_prefix('–'));
                if let Some(tail) = after_sep {
                    let digits: String = tail.chars().take_while(char::is_ascii_digit).collect();
                    if !digits.is_empty() {
                        out.push(format!("{}{digits}", &line[start..digits_start]));
                        j += rest.len() - tail.len() + digits.len();
                    }
                }
                i = j;
            }
        }
    }
    out
}

/// What the `specs/README.md` index says: spec files, and the prefixes of specs
/// that are written (their file exists) and of specs still to write.
#[derive(Debug, Default)]
struct Index {
    files: BTreeSet<String>,
    written: BTreeSet<String>,
    unwritten: BTreeSet<String>,
}

fn is_prefix(token: &str) -> bool {
    token.chars().next().is_some_and(|c| c.is_ascii_uppercase())
        && token
            .chars()
            .all(|c| c.is_ascii_uppercase() || c.is_ascii_digit())
}

fn cells(line: &str) -> Vec<&str> {
    line.trim()
        .trim_matches('|')
        .split('|')
        .map(str::trim)
        .collect()
}

/// Parse the index table. The `File` and `Prefix` columns are found from the
/// header row, so adding a column cannot silently empty the index; a README
/// without such a header is an error.
fn index(root: &Path) -> Result<Index> {
    let mut idx = Index::default();
    let path = root.join("specs/README.md");
    if !path.is_file() {
        return Ok(idx);
    }
    let text = read(&path)?;
    let mut columns: Option<(usize, usize)> = None;
    for line in text.lines().filter(|l| l.trim_start().starts_with('|')) {
        let row = cells(line);
        let Some((file_col, prefix_col)) = columns else {
            let find = |name: &str| row.iter().position(|c| c.eq_ignore_ascii_case(name));
            if let (Some(f), Some(p)) = (find("File"), find("Prefix")) {
                columns = Some((f, p));
            }
            continue;
        };
        let (Some(file), Some(prefixes)) = (row.get(file_col), row.get(prefix_col)) else {
            continue;
        };
        let mut is_written = false;
        if spec_number(file).is_some() && file.ends_with(".md") {
            idx.files.insert(format!("specs/{file}"));
            is_written = root.join("specs").join(file).is_file();
        }
        // A prefix cell holds whole prefixes separated by commas or spaces; a
        // placeholder such as `P…` is not a prefix.
        for p in prefixes
            .split(|c: char| c == ',' || c.is_whitespace())
            .filter(|p| is_prefix(p))
        {
            if is_written {
                idx.written.insert(p.to_owned());
            } else {
                idx.unwritten.insert(p.to_owned());
            }
        }
    }
    if columns.is_none() {
        return Err(Error::Invalid(
            "specs/README.md has no index table with `File` and `Prefix` columns".to_owned(),
        ));
    }
    Ok(idx)
}

fn has_ext(path: &Path, exts: &[&str]) -> bool {
    path.extension()
        .and_then(|x| x.to_str())
        .is_some_and(|x| exts.iter().any(|e| x.eq_ignore_ascii_case(e)))
}

fn refuse_symlink(root: &Path, path: &Path) -> Error {
    Error::Invalid(format!(
        "symlink in a scanned tree: {} (its target would escape the reference check)",
        rel(root, path)
    ))
}

fn scanned_files(root: &Path) -> Result<Vec<PathBuf>> {
    const MARKDOWN: [&str; 2] = ["md", "markdown"];
    let mut files = Vec::new();
    let mut top = Vec::new();
    for entry in std::fs::read_dir(root).map_err(|e| Error::io(root, e))? {
        top.push(entry.map_err(|e| Error::io(root, e))?.path());
    }
    top.sort();
    for p in top.into_iter().filter(|p| has_ext(p, &MARKDOWN)) {
        let meta = std::fs::symlink_metadata(&p).map_err(|e| Error::io(&p, e))?;
        if meta.file_type().is_symlink() {
            return Err(refuse_symlink(root, &p));
        }
        if meta.is_file() {
            files.push(p);
        }
    }
    for (base, exts) in [
        ("docs", &MARKDOWN[..]),
        ("specs", &MARKDOWN[..]),
        (".github", &MARKDOWN[..]),
        ("hypotheses", &["toml"][..]),
    ] {
        let dir = root.join(base);
        if !dir.is_dir() {
            continue;
        }
        let walker = WalkDir::new(&dir)
            .sort_by_file_name()
            .into_iter()
            .filter_entry(|e| !EXEMPT_DIRS.contains(&rel(root, e.path()).as_str()));
        for entry in walker {
            let entry = entry?;
            if entry.path_is_symlink() {
                return Err(refuse_symlink(root, entry.path()));
            }
            if entry.file_type().is_file() && has_ext(entry.path(), exts) {
                files.push(entry.path().to_path_buf());
            }
        }
    }
    Ok(files)
}

/// `[poc].spec` of a hypothesis file, read as TOML.
#[derive(Debug, PartialEq, Eq)]
enum SpecKey {
    Absent,
    Path(String),
    WrongType,
}

fn poc_spec(text: &str) -> std::result::Result<SpecKey, toml::de::Error> {
    let value: toml::Value = toml::from_str(text.trim_start_matches('\u{feff}'))?;
    Ok(match value.get("poc").and_then(|poc| poc.get("spec")) {
        None => SpecKey::Absent,
        Some(toml::Value::String(s)) => SpecKey::Path(s.clone()),
        Some(_) => SpecKey::WrongType,
    })
}

/// A spec path is `specs/<NNN>-<name>.md`: a numbered spec directly under `specs/`.
fn is_spec_path(spec: &str) -> bool {
    spec.strip_prefix("specs/").is_some_and(|f| {
        !f.contains('/') && !f.contains("..") && f.ends_with(".md") && spec_number(f).is_some()
    })
}

/// Scan `root` for ID references and classify them against `reqs`.
pub fn scan(root: &Path, reqs: &[Requirement]) -> Result<RefScan> {
    let known_ids: BTreeSet<&str> = reqs.iter().map(|r| r.id.as_str()).collect();
    let idx = index(root)?;
    // A prefix is written when a spec defines an ID with it, or when the spec
    // file the index assigns it to exists: a written spec with no definitions
    // must not leave its references classified as harmless forward references.
    let mut written: BTreeSet<&str> = reqs.iter().map(|r| r.prefix.as_str()).collect();
    written.extend(idx.written.iter().map(String::as_str));

    let mut out = RefScan::default();
    for path in scanned_files(root)? {
        let file = rel(root, &path);
        let text = read(&path)?;
        out.files_scanned += 1;
        // Inside specs/, fenced code is example text, under the same fence rules
        // as the spec parser. Elsewhere fences hold real references (TASKS.md).
        let skip_fences = file.starts_with("specs/");
        let mut fences = Fences::default();
        for (n, line) in text.lines().enumerate() {
            if skip_fences && fences.hides(line, n + 1) {
                continue;
            }
            for id in id_tokens(line) {
                let prefix = id.split_once('-').map_or("", |(p, _)| p);
                if written.contains(prefix) {
                    if !known_ids.contains(id.as_str()) {
                        out.dangling.push(Reference {
                            id,
                            file: file.clone(),
                            line: n + 1,
                        });
                    }
                } else if idx.unwritten.contains(prefix) {
                    out.forward.insert(id);
                }
            }
        }
        if file.starts_with("hypotheses/") {
            let problem = |spec: String, reason: &str| DanglingSpecFile {
                spec,
                file: file.clone(),
                reason: reason.to_owned(),
            };
            match poc_spec(&text) {
                Ok(SpecKey::Absent) => {}
                Ok(SpecKey::WrongType) => out
                    .dangling_spec_files
                    .push(problem("<not a string>".to_owned(), "`[poc].spec` must be a string")),
                Ok(SpecKey::Path(spec)) if !is_spec_path(&spec) => out.dangling_spec_files.push(problem(
                    spec,
                    "`[poc].spec` must be a numbered spec directly under specs/ (`specs/<NNN>-<name>.md`)",
                )),
                Ok(SpecKey::Path(spec)) if root.join(&spec).is_file() => {}
                Ok(SpecKey::Path(spec)) if idx.files.contains(&spec) => {
                    out.forward.insert(spec);
                }
                Ok(SpecKey::Path(spec)) => out.dangling_spec_files.push(problem(
                    spec,
                    "the spec file does not exist and is not listed in specs/README.md",
                )),
                Err(e) => out
                    .dangling_spec_files
                    .push(problem("<unparseable TOML>".to_owned(), &e.to_string())),
            }
        }
    }
    Ok(out)
}

#[cfg(test)]
mod tests {
    use super::*;

    /// Cites: CON-12
    #[test]
    fn tokens() {
        assert_eq!(
            id_tokens("see CON-5(e), TRC-10..21 and CON-5c; (LOOP-3), HYP-1–9"),
            [
                "CON-5", "TRC-10", "TRC-21", "CON-5", "LOOP-3", "HYP-1", "HYP-9"
            ]
        );
        assert_eq!(
            id_tokens("x86-64 aCON-1 pre-CON-2 UTF-8 P1A-3"),
            ["UTF-8", "P1A-3"]
        );
        assert_eq!(id_tokens("FIX-1.. and FIX-2–x"), ["FIX-1", "FIX-2"]);
        assert!(id_tokens("L-M0 H- -5 CON-").is_empty());
    }

    /// Cites: CON-12
    #[test]
    fn poc_spec_is_read_as_toml_and_typed() {
        let path = |s: &str| SpecKey::Path(s.to_owned());
        assert_eq!(
            poc_spec("[poc]\nspec = 'specs/100-a.md'\n").ok(),
            Some(path("specs/100-a.md"))
        );
        assert_eq!(
            poc_spec("poc = { spec = \"specs/100-b.md\" }\n").ok(),
            Some(path("specs/100-b.md"))
        );
        assert_eq!(
            poc_spec("\u{feff}[poc]\nid = \"p1\"\n").ok(),
            Some(SpecKey::Absent)
        );
        assert_eq!(
            poc_spec("[other]\nspec = \"specs/100-c.md\"\n").ok(),
            Some(SpecKey::Absent)
        );
        assert_eq!(
            poc_spec("[poc]\nspec = [\"specs/100-d.md\"]\n").ok(),
            Some(SpecKey::WrongType)
        );
        assert_eq!(
            poc_spec("[poc]\nspec = 42\n").ok(),
            Some(SpecKey::WrongType)
        );
        assert!(poc_spec("[poc\n").is_err());
    }

    /// Cites: CON-12
    #[test]
    fn spec_paths() {
        assert!(is_spec_path("specs/100-p4.md"));
        for bad in [
            "specs/../hypotheses/p1.toml",
            "specs/sub/100-x.md",
            "elsewhere/100-x.md",
            "specs/README.md",
            "specs/100-x.toml",
            "specs/",
        ] {
            assert!(!is_spec_path(bad), "{bad}");
        }
    }

    /// Cites: CON-12
    #[test]
    fn the_index_is_read_by_column_name_and_knows_which_specs_are_written() {
        let dir = tempfile::tempdir().expect("tempdir");
        let specs = dir.path().join("specs");
        std::fs::create_dir(&specs).expect("mkdir");
        std::fs::write(specs.join("010-written.md"), "no definitions yet\n").expect("write");
        std::fs::write(
            specs.join("README.md"),
            "# Index\n\n| # | Status | File | Prefix |\n|---|---|---|---|\n| 010 | draft | 010-written.md | WRT |\n| 120 | to write | 120-x.md | P11, P12 |\n| 140+ | later | one per POC | P… |\n| short |\n",
        )
        .expect("write");
        let idx = index(dir.path()).expect("index");
        assert_eq!(idx.written.iter().collect::<Vec<_>>(), ["WRT"]);
        assert_eq!(idx.unwritten.iter().collect::<Vec<_>>(), ["P11", "P12"]);
        assert_eq!(
            idx.files.iter().collect::<Vec<_>>(),
            ["specs/010-written.md", "specs/120-x.md"]
        );

        std::fs::write(
            specs.join("README.md"),
            "# Index\n\n| a | b |\n|---|---|\n| 1 | 2 |\n",
        )
        .expect("write");
        assert!(
            index(dir.path()).is_err(),
            "an index without File/Prefix columns is an error"
        );

        let none = tempfile::tempdir().expect("tempdir");
        assert!(index(none.path()).expect("no README").files.is_empty());
    }
}
