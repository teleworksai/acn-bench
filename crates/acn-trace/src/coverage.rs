//! Report coverage (TRC-36): every Appendix A key of SPEC 010 maps to exactly one
//! of a view column, a promoted attribute, an expression over view columns, or a
//! stated absence with the side-channel that holds the data instead. The mapping
//! lives in `docs/report/coverage.toml`, outside `crates/`, so it moves neither
//! `env_hash` nor `build_hash`; `cargo xtask docs-inventory` checks and renders it.

use std::collections::BTreeSet;

use serde::Deserialize;

use crate::schema::{Inventory, Views};

/// The mapping file, relative to the workspace root.
pub const COVERAGE_FILE: &str = "docs/report/coverage.toml";
/// The spec whose Appendix A lists the keys.
pub const SPEC_FILE: &str = "specs/010-trace-schema.md";

/// A mapping that does not cover the report.
#[derive(Debug, thiserror::Error)]
pub enum CoverageError {
    #[error("{COVERAGE_FILE}: {0}")]
    Parse(String),
    #[error("{0}")]
    Invalid(String),
}

type Result<T> = std::result::Result<T, CoverageError>;

fn invalid<T>(message: impl Into<String>) -> Result<T> {
    Err(CoverageError::Invalid(message.into()))
}

/// How a key is covered.
#[derive(Debug, Clone, Copy, PartialEq, Eq, Deserialize)]
#[serde(rename_all = "snake_case")]
pub enum Kind {
    Column,
    Attribute,
    Derived,
    NotRecorded,
}

/// One entry of `coverage.toml`. Which fields an entry carries depends on its
/// kind, and the loader requires exactly those.
#[derive(Debug, Clone, Deserialize)]
#[serde(deny_unknown_fields)]
pub struct Entry {
    pub key: String,
    pub kind: Kind,
    /// `column`: the view and the column.
    #[serde(default)]
    pub view: Option<String>,
    #[serde(default)]
    pub column: Option<String>,
    /// `attribute`: a promoted `acn.*` attribute.
    #[serde(default)]
    pub attribute: Option<String>,
    /// `derived`: the expression, and every view column it reads as `view.column`.
    #[serde(default)]
    pub expression: Option<String>,
    #[serde(default)]
    pub columns: Vec<String>,
    /// `not_recorded`: why, and where the data is instead.
    #[serde(default)]
    pub reason: Option<String>,
    #[serde(default)]
    pub side_channel: Option<String>,
}

#[derive(Debug, Deserialize)]
#[serde(deny_unknown_fields)]
struct CoverageFile {
    report_version: String,
    #[serde(rename = "key", default)]
    keys: Vec<Entry>,
}

/// The parsed and checked mapping.
#[derive(Debug, Clone)]
pub struct Coverage {
    pub report_version: String,
    pub entries: Vec<Entry>,
}

/// The Appendix A keys of SPEC 010: the first back-ticked cell of each table row
/// after the `## Appendix A` heading, in order.
pub fn appendix_a_keys(spec: &str) -> Result<Vec<String>> {
    let Some(start) = spec.find("## Appendix A") else {
        return invalid(format!("{SPEC_FILE} has no `## Appendix A`"));
    };
    let mut keys = Vec::new();
    for line in spec[start..].lines().skip(1) {
        if line.starts_with("## ") {
            break;
        }
        let Some(rest) = line.strip_prefix("| `") else {
            continue;
        };
        if let Some((key, _)) = rest.split_once('`') {
            keys.push(key.to_owned());
        }
    }
    if keys.is_empty() {
        return invalid(format!("{SPEC_FILE} Appendix A lists no keys"));
    }
    Ok(keys)
}

fn column_exists(views: &Views, view: &str, column: &str) -> bool {
    views
        .iter()
        .any(|v| v.name == view && v.column(column).is_some())
}

impl Coverage {
    /// Parse `coverage.toml` and check it against the Appendix A `keys`, the view
    /// schema (TRC-37) and the inventory (TRC-20).
    pub fn parse(text: &str, keys: &[String], views: &Views, inv: &Inventory) -> Result<Self> {
        let file: CoverageFile =
            toml::from_str(text).map_err(|e| CoverageError::Parse(e.to_string()))?;
        if file.report_version.is_empty() {
            return invalid("report_version must name the report version mapped");
        }
        let mut seen = BTreeSet::new();
        for e in &file.keys {
            let at = format!("`{}`", e.key);
            if !keys.contains(&e.key) {
                return invalid(format!("{at} is not an Appendix A key of {SPEC_FILE}"));
            }
            if !seen.insert(e.key.as_str()) {
                return invalid(format!("{at} is mapped twice"));
            }
            let has = |o: &Option<String>| o.as_deref().is_some_and(|s| !s.is_empty());
            let fields = [
                ("view", has(&e.view)),
                ("column", has(&e.column)),
                ("attribute", has(&e.attribute)),
                ("expression", has(&e.expression)),
                ("columns", !e.columns.is_empty()),
                ("reason", has(&e.reason)),
                ("side_channel", has(&e.side_channel)),
            ];
            let needed: &[&str] = match e.kind {
                Kind::Column => &["view", "column"],
                Kind::Attribute => &["attribute"],
                Kind::Derived => &["expression", "columns"],
                Kind::NotRecorded => &["reason", "side_channel"],
            };
            for (name, present) in fields {
                if present != needed.contains(&name) {
                    return invalid(format!(
                        "{at} ({:?}) {} `{name}`",
                        e.kind,
                        if present {
                            "must not carry"
                        } else {
                            "must carry"
                        }
                    ));
                }
            }
            match e.kind {
                Kind::Column => {
                    let (view, column) = (
                        e.view.as_deref().unwrap_or(""),
                        e.column.as_deref().unwrap_or(""),
                    );
                    if !column_exists(views, view, column) {
                        return invalid(format!(
                            "{at} names `{view}.{column}`, which views.toml does not define (TRC-37)"
                        ));
                    }
                }
                Kind::Attribute => {
                    let name = e.attribute.as_deref().unwrap_or("");
                    if !inv.attribute(name).is_some_and(|a| a.promoted) {
                        return invalid(format!(
                            "{at} names `{name}`, which acn_attributes.toml does not list as promoted (TRC-20)"
                        ));
                    }
                }
                Kind::Derived => {
                    for c in &e.columns {
                        let Some((view, column)) = c.split_once('.') else {
                            return invalid(format!("{at}: `{c}` is not `view.column`"));
                        };
                        if !column_exists(views, view, column) {
                            return invalid(format!(
                                "{at} reads `{c}`, which views.toml does not define (TRC-37)"
                            ));
                        }
                    }
                }
                Kind::NotRecorded => {}
            }
        }
        if let Some(missing) = keys.iter().find(|k| !seen.contains(k.as_str())) {
            return invalid(format!(
                "Appendix A key `{missing}` has no entry in {COVERAGE_FILE}"
            ));
        }
        // Keep the mapping in Appendix A order, so the rendered page reads like it.
        let mut entries = file.keys;
        entries.sort_by_key(|e| keys.iter().position(|k| *k == e.key));
        Ok(Self {
            report_version: file.report_version,
            entries,
        })
    }

    /// The `docs/generated/report-coverage.md` page body.
    #[must_use]
    pub fn page(&self) -> String {
        let esc = |s: &str| s.replace('|', "\\|");
        let mut s = format!(
            "# Report coverage\n\nHow each field of the report's §3.5 methodology and each parameter of its Appendix C sheet (report {}) is recorded, as `{COVERAGE_FILE}` maps the keys of SPEC 010 Appendix A (TRC-36).\n\n| Key | Kind | Where |\n|---|---|---|\n",
            esc(&self.report_version)
        );
        for e in &self.entries {
            let o = |v: &Option<String>| esc(v.as_deref().unwrap_or(""));
            let (kind, where_) = match e.kind {
                Kind::Column => ("column", format!("`{}.{}`", o(&e.view), o(&e.column))),
                Kind::Attribute => ("attribute", format!("`{}`", o(&e.attribute))),
                Kind::Derived => (
                    "derived",
                    format!(
                        "{} (reads {})",
                        o(&e.expression),
                        e.columns
                            .iter()
                            .map(|c| format!("`{c}`"))
                            .collect::<Vec<_>>()
                            .join(", ")
                    ),
                ),
                Kind::NotRecorded => (
                    "not recorded",
                    format!("{}; instead: {}", o(&e.reason), o(&e.side_channel)),
                ),
            };
            s.push_str(&format!("| `{}` | {kind} | {where_} |\n", e.key));
        }
        s
    }
}
