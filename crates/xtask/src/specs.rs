//! Requirement-ID extraction from `specs/<NNN>-*.md` (CON-12).
//!
//! A requirement is a paragraph that starts with a bold ID such as `**CON-5**`.
//! Its level is the strongest RFC 2119 keyword in the paragraph. IDs inside
//! fenced code blocks are ignored, and only the `## N.` heading in force gives
//! the section.

use std::path::Path;

use serde::Serialize;

use crate::workspace::read;
use crate::{Error, Result};

/// RFC 2119 level of a requirement paragraph.
#[derive(Debug, Clone, Copy, PartialEq, Eq, Serialize)]
#[serde(rename_all = "UPPERCASE")]
pub enum Level {
    Must,
    Should,
    May,
    None,
}

/// One requirement ID as defined in a spec.
#[derive(Debug, Clone, Serialize)]
pub struct Requirement {
    pub id: String,
    pub prefix: String,
    /// Spec number, e.g. `"000"`.
    pub spec: String,
    /// Spec file name, e.g. `"000-constitution.md"`.
    pub file: String,
    /// Section token from the enclosing `## N.` heading, if any.
    pub section: Option<String>,
    pub line: usize,
    pub level: Level,
}

/// Whether `token` has the shape of a requirement ID: `PREFIX-n`.
pub fn is_id(token: &str) -> bool {
    let Some((prefix, number)) = token.split_once('-') else {
        return false;
    };
    let prefix_ok = prefix
        .chars()
        .next()
        .is_some_and(|c| c.is_ascii_uppercase())
        && prefix
            .chars()
            .all(|c| c.is_ascii_uppercase() || c.is_ascii_digit());
    let number_ok = !number.is_empty() && number.chars().all(|c| c.is_ascii_digit());
    prefix_ok && number_ok
}

/// The spec number of a file name like `010-trace-schema.md`.
pub fn spec_number(file_name: &str) -> Option<&str> {
    let (num, _) = file_name.split_once('-')?;
    (!num.is_empty() && num.chars().all(|c| c.is_ascii_digit())).then_some(num)
}

fn level_of(paragraph: &str) -> Level {
    let mut level = Level::None;
    for word in paragraph.split(|c: char| !c.is_ascii_alphabetic()) {
        match word {
            "MUST" => return Level::Must,
            "SHOULD" => level = Level::Should,
            "MAY" if level == Level::None => level = Level::May,
            _ => {}
        }
    }
    level
}

fn section_token(heading: &str) -> Option<String> {
    let token: String = heading
        .trim()
        .chars()
        .take_while(|c| !c.is_whitespace() && *c != '.')
        .collect();
    (!token.is_empty() && token.chars().next().is_some_and(|c| c.is_ascii_digit())).then_some(token)
}

/// A CommonMark fence opener or closer: its character and run length.
fn fence(line: &str) -> Option<(char, usize)> {
    let t = line.trim_start();
    let c = t.chars().next().filter(|c| *c == '`' || *c == '~')?;
    let n = t.chars().take_while(|x| *x == c).count();
    (n >= 3).then_some((c, n))
}

/// Fence state for a Markdown file, shared by the spec parser and the reference
/// scanner so that both agree on which lines are example text.
#[derive(Debug, Default)]
pub(crate) struct Fences {
    /// (fence character, opening run length, opening line)
    open: Option<(char, usize, usize)>,
}

impl Fences {
    /// Whether `line` is a fence delimiter or lies inside a fence. A fence closes
    /// only on the same character, at least as long, with nothing after it.
    pub(crate) fn hides(&mut self, line: &str, line_no: usize) -> bool {
        if let Some((c, n)) = fence(line) {
            match self.open {
                None => self.open = Some((c, n, line_no)),
                Some((oc, on, _)) if c == oc && n >= on && line.trim().chars().all(|x| x == c) => {
                    self.open = None;
                }
                Some(_) => {}
            }
            return true;
        }
        self.open.is_some()
    }

    /// The line on which a still-open fence started.
    pub(crate) fn unclosed(&self) -> Option<usize> {
        self.open.map(|(_, _, line)| line)
    }
}

/// The bold ID that starts a requirement paragraph, optionally behind a list marker.
fn leading_id(line: &str) -> Option<&str> {
    let t = line.trim_start();
    let t = t
        .strip_prefix("- ")
        .or_else(|| t.strip_prefix("* "))
        .unwrap_or(t)
        .trim_start();
    let rest = t.strip_prefix("**")?;
    let token = &rest[..rest.find("**")?];
    is_id(token).then_some(token)
}

/// Parse one spec's text. Pure, for unit tests. An unclosed code fence is an
/// error: it would silently hide every requirement after it.
pub fn parse_spec_text(file: &str, spec: &str, text: &str) -> Result<Vec<Requirement>> {
    let text = text.strip_prefix('\u{feff}').unwrap_or(text);
    let lines: Vec<&str> = text.lines().collect();
    let mut out = Vec::new();
    let mut fences = Fences::default();
    let mut section: Option<String> = None;
    for (i, line) in lines.iter().enumerate() {
        if fences.hides(line, i + 1) {
            continue;
        }
        let t = line.trim_start();
        if let Some(h) = t.strip_prefix("## ") {
            section = section_token(h);
            continue;
        }
        let Some(token) = leading_id(line) else {
            continue;
        };
        let prefix = token.split_once('-').map_or("", |(p, _)| p);
        let mut paragraph = (*line).to_owned();
        for l in &lines[i + 1..] {
            if l.trim().is_empty() {
                break;
            }
            paragraph.push(' ');
            paragraph.push_str(l);
        }
        out.push(Requirement {
            id: token.to_owned(),
            prefix: prefix.to_owned(),
            spec: spec.to_owned(),
            file: file.to_owned(),
            section: section.clone(),
            line: i + 1,
            level: level_of(&paragraph),
        });
    }
    if let Some(line) = fences.unclosed() {
        return Err(Error::Invalid(format!(
            "specs/{file}:{line}: code fence is never closed, which would hide every requirement after it"
        )));
    }
    Ok(out)
}

/// Spec files under `<root>/specs/` that carry a numeric prefix, sorted.
pub fn spec_files(root: &Path) -> Result<Vec<(String, String)>> {
    let dir = root.join("specs");
    if !dir.is_dir() {
        return Err(Error::Invalid(format!(
            "no specs/ directory under {} (pass --root <workspace>)",
            root.display()
        )));
    }
    let mut files = Vec::new();
    for entry in std::fs::read_dir(&dir).map_err(|e| Error::io(&dir, e))? {
        let entry = entry.map_err(|e| Error::io(&dir, e))?;
        let name = entry.file_name().to_string_lossy().into_owned();
        if !name.ends_with(".md") {
            continue;
        }
        if let Some(num) = spec_number(&name) {
            files.push((num.to_owned(), name));
        }
    }
    files.sort();
    if files.is_empty() {
        return Err(Error::Invalid(format!(
            "no <NNN>-*.md spec files under {}",
            dir.display()
        )));
    }
    Ok(files)
}

/// All requirements defined under `<root>/specs/`, in file then line order.
pub fn parse_specs(root: &Path) -> Result<Vec<Requirement>> {
    let mut out = Vec::new();
    for (num, name) in spec_files(root)? {
        let text = read(&root.join("specs").join(&name))?;
        let reqs = parse_spec_text(&name, &num, &text)?;
        for r in reqs {
            if let Some(dup) = out.iter().find(|x: &&Requirement| x.id == r.id) {
                return Err(Error::Invalid(format!(
                    "requirement {} is defined twice: {}:{} and {}:{}",
                    r.id, dup.file, dup.line, r.file, r.line
                )));
            }
            out.push(r);
        }
    }
    Ok(out)
}

#[cfg(test)]
mod tests {
    use super::*;

    fn parse(text: &str) -> Vec<Requirement> {
        parse_spec_text("900-x.md", "900", text).expect("parse")
    }

    fn specs_dir(files: &[(&str, &str)]) -> tempfile::TempDir {
        let dir = tempfile::tempdir().expect("tempdir");
        std::fs::create_dir(dir.path().join("specs")).expect("mkdir");
        for (name, body) in files {
            std::fs::write(dir.path().join("specs").join(name), body).expect("write");
        }
        dir
    }

    /// Cites: CON-12
    #[test]
    fn id_shape() {
        assert!(is_id("CON-5"));
        assert!(is_id("P1A-12"));
        assert!(
            is_id("X-99999999999"),
            "no numeric limit: the number is never parsed"
        );
        assert!(!is_id("con-5"));
        assert!(!is_id("CON-"));
        assert!(!is_id("Status:"));
        assert!(!is_id("L0-Build"));
    }

    /// Cites: CON-12
    #[test]
    fn levels_sections_and_list_items() {
        let reqs = parse(
            "\u{feff}**X-0** First line after a BOM MUST parse.\n\n## 2. Rules\n\n**X-1** It MUST NOT fail.\n\n**X-2** It MAY\ncontinue and SHOULD end.\n\n- **X-3** A list item MUST count.\n\n**X-4** plain text.\n",
        );
        let ids: Vec<&str> = reqs.iter().map(|r| r.id.as_str()).collect();
        assert_eq!(ids, ["X-0", "X-1", "X-2", "X-3", "X-4"]);
        assert_eq!(reqs[1].level, Level::Must);
        assert_eq!(reqs[2].level, Level::Should);
        assert_eq!(reqs[4].level, Level::None);
        assert_eq!(reqs[1].section.as_deref(), Some("2"));
        assert_eq!(reqs[1].prefix, "X");
    }

    /// Cites: CON-12
    #[test]
    fn fences_hide_only_what_they_enclose() {
        let text = "**X-1** before MUST.\n\n````md\n**X-2** fenced MUST\n```\nstill fenced: a shorter run does not close a longer fence\n**X-3** fenced MUST\n````\n\n**X-4** after MUST.\n\n~~~\n**X-5** tilde fenced MUST\n~~~\n\n**X-6** last MUST.\n";
        let ids: Vec<String> = parse(text).into_iter().map(|r| r.id).collect();
        assert_eq!(ids, ["X-1", "X-4", "X-6"]);
    }

    /// Cites: CON-12
    #[test]
    fn an_unclosed_fence_is_an_error_not_a_silent_truncation() {
        let err = parse_spec_text(
            "900-x.md",
            "900",
            "**X-1** a MUST.\n\n```\n**X-2** hidden MUST\n",
        )
        .expect_err("unclosed fence");
        assert!(err.to_string().contains("900-x.md:3"), "{err}");
    }

    /// Cites: CON-12
    #[test]
    fn spec_numbers() {
        assert_eq!(spec_number("000-constitution.md"), Some("000"));
        assert_eq!(spec_number("README.md"), None);
    }

    /// Cites: CON-12
    #[test]
    fn duplicate_definitions_are_an_error_within_and_across_files() {
        let one = specs_dir(&[("900-x.md", "**X-1** It MUST a.\n\n**X-1** It MUST b.\n")]);
        let err = parse_specs(one.path()).expect_err("duplicate must fail");
        assert!(err.to_string().contains("defined twice"), "{err}");

        let two = specs_dir(&[
            ("900-a.md", "**X-1** It MUST a.\n"),
            ("901-b.md", "**X-1** It MUST b.\n"),
        ]);
        let msg = parse_specs(two.path())
            .expect_err("duplicate must fail")
            .to_string();
        assert!(
            msg.contains("900-a.md") && msg.contains("901-b.md"),
            "{msg}"
        );
    }

    /// Cites: CON-12
    #[test]
    fn a_root_with_no_spec_files_is_an_error() {
        let none = tempfile::tempdir().expect("tempdir");
        let err = parse_specs(none.path()).expect_err("no specs/");
        assert!(err.to_string().contains("no specs/ directory"), "{err}");

        let readme_only = specs_dir(&[("README.md", "# index\n")]);
        let err = parse_specs(readme_only.path()).expect_err("no numbered specs");
        assert!(
            err.to_string().contains("no <NNN>-*.md spec files"),
            "{err}"
        );
    }
}
