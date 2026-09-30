//! `/// Cites: ID, ID` extraction from Rust sources (CON-12, ADR-3 amendment 3).
//!
//! The scanner parses Rust with `syn`; it does not read text. It starts from the
//! crate roots that cargo would build for each workspace member (`src/lib.rs`,
//! `src/main.rs`, `[[bin]]`/`[[test]]` paths, top-level `tests/*.rs`) and follows
//! `mod` declarations, so only code the compiler sees can count. A citation is a
//! doc-comment line starting with `Cites:` (a line ending in `,` continues on the
//! next doc line) on a `fn` that carries a test attribute: an attribute whose
//! path ends in `test`, directly or as an attribute applied by `cfg_attr`.
//! Commented-out code, string literals, macro bodies, orphan files and files
//! that do not parse can therefore never satisfy a requirement. `lab/` is not a
//! workspace member and is never scanned (CON-23).

use std::collections::BTreeSet;
use std::path::{Path, PathBuf};

use serde::Serialize;
use syn::punctuated::Punctuated;
use syn::spanned::Spanned;
use syn::visit::Visit;

use crate::specs::is_id;
use crate::workspace::{read, rel};
use crate::{Error, Result};

const MARKER: &str = "Cites:";

/// One citation of one ID from one test function.
#[derive(Debug, Clone, Serialize)]
pub struct Citation {
    pub id: String,
    pub file: String,
    pub line: usize,
    pub function: Option<String>,
}

/// A malformed, misplaced or unparseable citation site.
#[derive(Debug, Clone, Serialize)]
pub struct Problem {
    pub file: String,
    pub line: usize,
    pub message: String,
}

/// Everything found in a scan.
#[derive(Debug, Default, Serialize)]
pub struct Scan {
    pub citations: Vec<Citation>,
    pub problems: Vec<Problem>,
    pub files_scanned: usize,
}

/// The text of a `#[doc = "…"]` attribute (what `///` desugars to).
fn doc_text(attr: &syn::Attribute) -> Option<String> {
    if !attr.path().is_ident("doc") {
        return None;
    }
    match &attr.meta {
        syn::Meta::NameValue(nv) => match &nv.value {
            syn::Expr::Lit(syn::ExprLit {
                lit: syn::Lit::Str(s),
                ..
            }) => Some(s.value()),
            _ => None,
        },
        _ => None,
    }
}

fn ends_in_test(path: &syn::Path) -> bool {
    path.segments.last().is_some_and(|s| s.ident == "test")
}

/// `#[test]`, `#[tokio::test(...)]`, `#[test_log::test(...)]`, or a `cfg_attr`
/// that *applies* such an attribute. The `cfg_attr` predicate is skipped as a
/// parsed meta item, so `test` inside a predicate or a string never counts.
fn is_test_attr(attr: &syn::Attribute) -> bool {
    if ends_in_test(attr.path()) {
        return true;
    }
    if attr.path().is_ident("cfg_attr") {
        let applied =
            attr.parse_args_with(Punctuated::<syn::Meta, syn::Token![,]>::parse_terminated);
        if let Ok(metas) = applied {
            return metas.iter().skip(1).any(|m| ends_in_test(m.path()));
        }
    }
    false
}

fn has_marker(line: &str) -> bool {
    line.trim_start().starts_with(MARKER)
}

fn has_misspelled_marker(line: &str) -> bool {
    let t = line.trim_start();
    !t.starts_with(MARKER)
        && t.get(..MARKER.len())
            .is_some_and(|p| p.eq_ignore_ascii_case(MARKER))
}

/// (line number, text) for every doc line on an item, in source order. A block
/// doc comment contributes one entry per line.
fn doc_lines(attrs: &[syn::Attribute]) -> Vec<(usize, String)> {
    let mut out = Vec::new();
    for attr in attrs {
        if let Some(text) = doc_text(attr) {
            let first = attr.span().start().line;
            for (k, l) in text.split('\n').enumerate() {
                out.push((first + k, l.to_owned()));
            }
        }
    }
    out
}

struct Visitor<'a> {
    file: &'a str,
    out: &'a mut Scan,
    /// Lines of `Cites:` doc lines that sat on a function.
    on_function: BTreeSet<usize>,
    /// Lines of every `Cites:` doc line anywhere in the file.
    everywhere: BTreeSet<usize>,
}

impl Visitor<'_> {
    fn problem(&mut self, line: usize, message: String) {
        self.out.problems.push(Problem {
            file: self.file.to_owned(),
            line,
            message,
        });
    }

    fn function(&mut self, attrs: &[syn::Attribute], name: &str) {
        let is_test = attrs.iter().any(is_test_attr);
        let docs = doc_lines(attrs);
        let mut i = 0;
        while i < docs.len() {
            let (line, text) = &docs[i];
            if !has_marker(text) {
                i += 1;
                continue;
            }
            let line = *line;
            self.on_function.insert(line);
            let mut list = text.trim_start()[MARKER.len()..].trim().to_owned();
            i += 1;
            while list.ends_with(',') && i < docs.len() && !has_marker(&docs[i].1) {
                list.push(' ');
                list.push_str(docs[i].1.trim());
                i += 1;
            }
            let ids: Vec<&str> = list
                .split(|c: char| c == ',' || c.is_whitespace())
                .filter(|s| !s.is_empty())
                .collect();
            if ids.is_empty() {
                self.problem(line, "empty `Cites:` list".to_owned());
            }
            if !is_test {
                self.problem(
                    line,
                    format!(
                        "citation is on `{name}`, which is not a test function (no `#[test]`-style attribute)"
                    ),
                );
            }
            for id in ids {
                if !is_id(id) {
                    self.problem(
                        line,
                        format!("malformed requirement ID `{id}` (expected PREFIX-n)"),
                    );
                } else if is_test {
                    self.out.citations.push(Citation {
                        id: id.to_owned(),
                        file: self.file.to_owned(),
                        line,
                        function: Some(name.to_owned()),
                    });
                }
            }
        }
    }
}

impl<'ast> Visit<'ast> for Visitor<'_> {
    fn visit_attribute(&mut self, attr: &'ast syn::Attribute) {
        if let Some(text) = doc_text(attr) {
            let first = attr.span().start().line;
            for (k, l) in text.split('\n').enumerate() {
                if has_marker(l) {
                    self.everywhere.insert(first + k);
                } else if has_misspelled_marker(l) {
                    self.problem(
                        first + k,
                        "citation marker must be spelled exactly `Cites:`".to_owned(),
                    );
                }
            }
        }
        syn::visit::visit_attribute(self, attr);
    }

    fn visit_item_fn(&mut self, node: &'ast syn::ItemFn) {
        self.function(&node.attrs, &node.sig.ident.to_string());
        syn::visit::visit_item_fn(self, node);
    }

    fn visit_impl_item_fn(&mut self, node: &'ast syn::ImplItemFn) {
        self.function(&node.attrs, &node.sig.ident.to_string());
        syn::visit::visit_impl_item_fn(self, node);
    }

    fn visit_trait_item_fn(&mut self, node: &'ast syn::TraitItemFn) {
        self.function(&node.attrs, &node.sig.ident.to_string());
        syn::visit::visit_trait_item_fn(self, node);
    }
}

/// Scan one file's text. Returns the parsed file so the caller can follow its
/// `mod` declarations; `None` when the text is not valid Rust (reported as a problem).
pub fn scan_text(file: &str, text: &str, out: &mut Scan) -> Option<syn::File> {
    let text = text.strip_prefix('\u{feff}').unwrap_or(text);
    let ast = match syn::parse_file(text) {
        Ok(ast) => ast,
        Err(e) => {
            out.problems.push(Problem {
                file: file.to_owned(),
                line: e.span().start().line,
                message: format!(
                    "file does not parse as Rust, so nothing in it can cite a requirement: {e}"
                ),
            });
            return None;
        }
    };
    let mut v = Visitor {
        file,
        out,
        on_function: BTreeSet::new(),
        everywhere: BTreeSet::new(),
    };
    v.visit_file(&ast);
    let detached: Vec<usize> = v.everywhere.difference(&v.on_function).copied().collect();
    for line in detached {
        v.problem(line, "citation is not attached to a function".to_owned());
    }
    Some(ast)
}

fn path_attr(attrs: &[syn::Attribute]) -> Option<String> {
    attrs
        .iter()
        .find(|a| a.path().is_ident("path"))
        .and_then(|a| match &a.meta {
            syn::Meta::NameValue(nv) => match &nv.value {
                syn::Expr::Lit(syn::ExprLit {
                    lit: syn::Lit::Str(s),
                    ..
                }) => Some(s.value()),
                _ => None,
            },
            _ => None,
        })
}

struct Walk<'a> {
    root: &'a Path,
    seen: BTreeSet<PathBuf>,
    out: Scan,
}

impl Walk<'_> {
    /// Parse `file` and follow its out-of-line modules. `children` is the
    /// directory in which `mod name;` is looked up.
    fn file(&mut self, file: &Path, children: &Path) -> Result<()> {
        if !self.seen.insert(file.to_path_buf()) {
            return Ok(());
        }
        let text = read(file)?;
        self.out.files_scanned += 1;
        let name = rel(self.root, file);
        if let Some(ast) = scan_text(&name, &text, &mut self.out) {
            let file_dir = file.parent().unwrap_or(self.root).to_path_buf();
            self.modules(&ast.items, &file_dir, children)?;
        }
        Ok(())
    }

    fn modules(&mut self, items: &[syn::Item], file_dir: &Path, children: &Path) -> Result<()> {
        for item in items {
            let syn::Item::Mod(m) = item else {
                continue;
            };
            let name = m.ident.to_string();
            if let Some((_, inline)) = &m.content {
                self.modules(inline, file_dir, &children.join(&name))?;
                continue;
            }
            let candidates = match path_attr(&m.attrs) {
                Some(p) => vec![file_dir.join(p)],
                None => vec![
                    children.join(format!("{name}.rs")),
                    children.join(&name).join("mod.rs"),
                ],
            };
            // A declared module with no file is a compile error, which `cargo test` reports.
            if let Some(found) = candidates.into_iter().find(|c| c.is_file()) {
                let is_mod_rs = found.file_name().is_some_and(|f| f == "mod.rs");
                let next = if is_mod_rs {
                    found.parent().unwrap_or(children).to_path_buf()
                } else {
                    found.with_extension("")
                };
                self.file(&found, &next)?;
            }
        }
        Ok(())
    }
}

fn toml_at(path: &Path) -> Result<toml::Value> {
    toml::from_str(&read(path)?).map_err(|e| Error::Toml {
        path: path.to_path_buf(),
        source: e,
    })
}

fn rs_files_in(dir: &Path) -> Result<Vec<PathBuf>> {
    let mut v = Vec::new();
    if dir.is_dir() {
        for entry in std::fs::read_dir(dir).map_err(|e| Error::io(dir, e))? {
            let p = entry.map_err(|e| Error::io(dir, e))?.path();
            if p.is_file() && p.extension().is_some_and(|x| x == "rs") {
                v.push(p);
            }
        }
    }
    v.sort();
    Ok(v)
}

/// The source files cargo would compile as crate roots for every workspace member.
pub fn crate_roots(root: &Path) -> Result<Vec<PathBuf>> {
    let manifest_path = root.join("Cargo.toml");
    if !manifest_path.is_file() {
        return Err(Error::Invalid(format!(
            "no Cargo.toml under {}: citations are read from the workspace's crates",
            root.display()
        )));
    }
    let manifest = toml_at(&manifest_path)?;
    let mut members: Vec<PathBuf> = Vec::new();
    match manifest
        .get("workspace")
        .and_then(|w| w.get("members"))
        .and_then(toml::Value::as_array)
    {
        Some(list) => {
            for m in list.iter().filter_map(toml::Value::as_str) {
                if let Some(parent) = m.strip_suffix("/*") {
                    let dir = root.join(parent);
                    let mut subs: Vec<PathBuf> = std::fs::read_dir(&dir)
                        .map_err(|e| Error::io(&dir, e))?
                        .filter_map(std::result::Result::ok)
                        .map(|e| e.path())
                        .filter(|p| p.join("Cargo.toml").is_file())
                        .collect();
                    subs.sort();
                    members.extend(subs);
                } else {
                    members.push(root.join(m));
                }
            }
        }
        None => members.push(root.to_path_buf()),
    }

    let mut roots = Vec::new();
    for dir in members {
        let m = toml_at(&dir.join("Cargo.toml"))?;
        let declared = |table: &str| -> Vec<PathBuf> {
            let one = m
                .get(table)
                .and_then(|t| t.get("path"))
                .and_then(toml::Value::as_str);
            let many = m
                .get(table)
                .and_then(toml::Value::as_array)
                .into_iter()
                .flatten()
                .filter_map(|t| t.get("path").and_then(toml::Value::as_str));
            one.into_iter().chain(many).map(|p| dir.join(p)).collect()
        };
        for p in [dir.join("src/lib.rs"), dir.join("src/main.rs")] {
            if p.is_file() {
                roots.push(p);
            }
        }
        roots.extend(declared("lib"));
        roots.extend(declared("bin"));
        roots.extend(declared("test"));
        roots.extend(rs_files_in(&dir.join("src/bin"))?);
        let autotests = m
            .get("package")
            .and_then(|p| p.get("autotests"))
            .and_then(toml::Value::as_bool)
            .unwrap_or(true);
        if autotests {
            roots.extend(rs_files_in(&dir.join("tests"))?);
        }
    }
    roots.sort();
    roots.dedup();
    Ok(roots)
}

/// Scan every workspace member's module tree for citations.
pub fn scan(root: &Path) -> Result<Scan> {
    let mut walk = Walk {
        root,
        seen: BTreeSet::new(),
        out: Scan::default(),
    };
    for crate_root in crate_roots(root)? {
        if crate_root.is_file() {
            let children = crate_root.parent().unwrap_or(root).to_path_buf();
            walk.file(&crate_root, &children)?;
        }
    }
    Ok(walk.out)
}

#[cfg(test)]
mod tests {
    use super::*;

    fn run(text: &str) -> Scan {
        let mut scan = Scan::default();
        scan_text("t.rs", text, &mut scan);
        scan
    }

    fn ids(s: &Scan) -> Vec<(&str, Option<&str>)> {
        s.citations
            .iter()
            .map(|c| (c.id.as_str(), c.function.as_deref()))
            .collect()
    }

    /// Cites: CON-12
    #[test]
    fn continuation_lines_multiline_attributes_and_attribute_order() {
        let s = run(
            "/// Cites: A-1,\n///   A-2\n/// more docs\n#[tokio::test(\n  start_paused = true\n)]\nasync fn go() {}\n\n#[test]\n// an ordinary comment between attribute and docs\n/// Cites: A-3\n/// Cites: A-4\nfn later() {}\n\n/// Cites: A-5\n#[test] fn same_line() { let _ = [0u8; 1]; }\n",
        );
        assert_eq!(
            ids(&s),
            [
                ("A-1", Some("go")),
                ("A-2", Some("go")),
                ("A-3", Some("later")),
                ("A-4", Some("later")),
                ("A-5", Some("same_line"))
            ]
        );
        assert!(s.problems.is_empty(), "{:?}", s.problems);
        assert_eq!(s.citations[0].line, 1);
        assert_eq!(s.citations[2].line, 11);
    }

    /// Cites: CON-12
    #[test]
    fn only_attributes_whose_path_ends_in_test_make_a_test() {
        let s = run(
            "#[cfg(test)]\n/// Cites: A-1\nfn helper() {}\n\n#[cfg(not(test))]\n/// Cites: A-2\npub fn prod() {}\n\n#[doc = \"a test\"]\n/// Cites: A-3\nfn documented() {}\n\n#[cfg_attr(feature = \"x\", ignore, test)]\n/// Cites: A-4\nfn conditional() {}\n\n#[rstest]\n/// Cites: A-5\nfn other_framework() {}\n\n#[cfg_attr(any(unix, test, windows), allow(dead_code))]\n/// Cites: A-6\npub fn predicate_mentions_test() {}\n\n#[cfg_attr(all(), doc = \"example, test , text\")]\n/// Cites: A-7\npub fn string_mentions_test() {}\n\n#[test_log::test(tokio::test)]\n/// Cites: A-8\nasync fn wrapped() {}\n",
        );
        assert_eq!(
            ids(&s),
            [("A-4", Some("conditional")), ("A-8", Some("wrapped"))]
        );
        assert_eq!(s.problems.len(), 6, "{:?}", s.problems);
        assert!(
            s.problems
                .iter()
                .all(|p| p.message.contains("not a test function"))
        );
    }

    /// Cites: CON-12
    #[test]
    fn code_the_compiler_never_sees_cannot_cite() {
        let s = run(
            "/*\n/// Cites: A-1\n#[test]\nfn commented_out() {}\n*/\n\n#[test]\nfn t() {\n    let _src = \"\n/// Cites: A-2\n#[test]\nfn inside_a_string() {}\n\";\n    let _raw = r#\"\n/// Cites: A-3\n#[test]\nfn inside_a_raw_string() {}\n\"#;\n}\n\nmacro_rules! m { () => {\n/// Cites: A-4\n#[test]\nfn inside_a_macro() {}\n}; }\n",
        );
        assert!(s.citations.is_empty(), "{:?}", s.citations);
        assert!(s.problems.is_empty(), "{:?}", s.problems);
    }

    /// Cites: CON-12
    #[test]
    fn an_earlier_test_attribute_is_never_borrowed() {
        let s = run(
            "#[tokio::test(flavor = \"current_thread\")]\nasync fn real() {}\nconst A: &[u8] = &[1\n];\n/// Cites: A-1\nfn helper() {}\n",
        );
        assert!(s.citations.is_empty(), "{:?}", s.citations);
        assert_eq!(s.problems.len(), 1, "{:?}", s.problems);
    }

    /// Cites: CON-12
    #[test]
    fn detached_empty_malformed_and_misspelled_are_problems() {
        let s = run(
            "/// Cites: B-1\nstruct S;\n\n/// Cites:\n#[test]\nfn empty() {}\n\n/// cites: B-3\n#[test]\nfn lower() {}\n\n/// Cites: b-4\n#[test]\nfn malformed() {}\n",
        );
        assert!(s.citations.is_empty(), "{:?}", s.citations);
        let msgs: Vec<&str> = s.problems.iter().map(|p| p.message.as_str()).collect();
        assert_eq!(msgs.len(), 4, "{msgs:?}");
        assert!(msgs.iter().any(|m| m.contains("not attached")));
        assert!(msgs.iter().any(|m| m.contains("empty")));
        assert!(msgs.iter().any(|m| m.contains("spelled exactly")));
        assert!(msgs.iter().any(|m| m.contains("malformed")));
    }

    /// Cites: CON-8, CON-12
    #[test]
    fn multibyte_doc_comments_and_a_bom_do_not_panic_or_hide_citations() {
        let s = run("\u{feff}/// Cites: A-1\n/// aaaaañ doc 🦀 — §\n#[test]\nfn t() {}\n");
        assert_eq!(ids(&s), [("A-1", Some("t"))]);
        assert!(s.problems.is_empty(), "{:?}", s.problems);
    }

    /// Cites: CON-12
    #[test]
    fn a_file_that_is_not_rust_is_a_problem_not_a_citation() {
        let s = run("/// Cites: A-1\n#[test]\nfn never_compiled() { this is not even rust }\n");
        assert!(s.citations.is_empty());
        assert_eq!(s.problems.len(), 1);
        assert!(s.problems[0].message.contains("does not parse"));
    }

    /// Cites: CON-12
    #[test]
    fn nested_and_impl_functions_are_judged_by_their_own_attributes() {
        let s = run(
            "#[test]\nfn outer() {\n    /// Cites: A-1\n    fn nested_helper() {}\n}\nstruct S;\nimpl S {\n    /// Cites: A-2\n    fn method(&self) {}\n}\n",
        );
        assert!(s.citations.is_empty(), "{:?}", s.citations);
        assert_eq!(s.problems.len(), 2, "{:?}", s.problems);
    }
}
