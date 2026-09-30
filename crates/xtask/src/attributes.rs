//! TRC-20: `docs/generated/acn-attributes.md` is rendered from the frozen
//! inventory, and no crate may emit an `acn.*` name the inventory does not list.
//!
//! The emission check reads source tokens, not text, so a name inside a macro
//! invocation (`info_span!("acn.session", …)`) is seen and a name inside a comment
//! is not. It looks at every `src/` tree under `crates/` and flags every string
//! literal that starts with `acn.` or `opt.` and is not a listed name: that catches a
//! name built with `format!` or `concat!` (the literal is then a prefix or a
//! template) and a misspelt one (`acn.Call.Index`) as well as an unlisted one.

use std::path::Path;

use acn_trace::schema::{Inventory, ValueType};
use proc_macro2::{TokenStream, TokenTree};
use serde::Serialize;
use walkdir::WalkDir;

use crate::workspace::{read, rel_strict};
use crate::{Error, Result};

/// Where the inventory lives, relative to the workspace root.
pub const SCHEMA_DIR: &str = "crates/acn-trace/src/schema";

/// An `acn.*` string literal that the inventory does not list.
#[derive(Debug, Clone, Serialize, PartialEq, Eq, PartialOrd, Ord)]
pub struct Unlisted {
    pub file: String,
    pub name: String,
}

/// The inventory of the workspace at `root`, or `None` when it has no schema
/// (the small test fixtures).
pub fn load(root: &Path) -> Result<Option<Inventory>> {
    let file = root.join(SCHEMA_DIR).join("acn_attributes.toml");
    if !file.is_file() {
        return Ok(None);
    }
    let pin = read(&root.join(SCHEMA_DIR).join("SEMCONV_VERSION"))?;
    Inventory::parse(&read(&file)?, &pin)
        .map(Some)
        .map_err(|e| Error::Invalid(e.to_string()))
}

/// The two files that must name the bare prefixes to do their job.
const PREFIX_USERS: &[&str] = &[
    "crates/acn-trace/src/schema/mod.rs",
    "crates/xtask/src/attributes.rs",
];

fn literals(stream: TokenStream, out: &mut Vec<String>) {
    for tree in stream {
        match tree {
            TokenTree::Group(g) => literals(g.stream(), out),
            TokenTree::Literal(lit) => {
                if let Ok(s) = syn::parse_str::<syn::LitStr>(&lit.to_string()) {
                    let value = s.value();
                    if value.starts_with("acn.") || value.starts_with("opt.") {
                        out.push(value);
                    }
                }
            }
            TokenTree::Ident(_) | TokenTree::Punct(_) => {}
        }
    }
}

/// Every `acn.` or `opt.` string literal under `crates/*/src` that `inv` does not list.
pub fn unlisted(root: &Path, inv: &Inventory) -> Result<Vec<Unlisted>> {
    let listed: std::collections::BTreeSet<&str> = inv
        .all_names()
        .chain(inv.options().iter().map(|o| o.name.as_str()))
        .collect();
    let mut found = Vec::new();
    let crates = root.join("crates");
    if !crates.is_dir() {
        return Ok(found);
    }
    for entry in WalkDir::new(&crates)
        .sort_by_file_name()
        .into_iter()
        // `crates/<crate>/tests` holds integration tests, which may use made-up names;
        // a `tests` directory deeper down (`src/tests/`) is source like any other.
        .filter_entry(|e| {
            e.file_name() != "target" && !(e.depth() == 2 && e.file_name() == "tests")
        })
    {
        let entry = entry?;
        let path = entry.path();
        if !entry.file_type().is_file() || path.extension().is_none_or(|x| x != "rs") {
            continue;
        }
        let rel = rel_strict(root, path)?;
        if !rel.contains("/src/") {
            continue;
        }
        let stream: TokenStream = read(path)?.parse().map_err(|e| {
            Error::Invalid(format!(
                "{rel}: cannot tokenise ({e}); the TRC-20 check reads every source file"
            ))
        })?;
        let mut names = Vec::new();
        literals(stream, &mut names);
        for name in names {
            let bare_prefix = name == "acn." || name == "opt.";
            if bare_prefix && PREFIX_USERS.contains(&rel.as_str()) {
                continue;
            }
            if !listed.contains(name.as_str()) {
                found.push(Unlisted {
                    file: rel.clone(),
                    name,
                });
            }
        }
    }
    found.sort();
    found.dedup();
    Ok(found)
}

fn ty(t: ValueType) -> &'static str {
    match t {
        ValueType::String => "string",
        ValueType::Int => "int",
        ValueType::Float => "float",
        ValueType::Bool => "bool",
        ValueType::Bytes => "bytes",
    }
}

fn cell(s: &str) -> String {
    if s.is_empty() {
        "—".to_owned()
    } else {
        s.replace('|', "\\|")
    }
}

/// The generated page.
#[must_use]
pub fn page(inv: &Inventory) -> String {
    let mut o = String::new();
    o.push_str("# `acn.*` attribute inventory (TRC-20)\n\n");
    o.push_str(&format!(
        "Source: `{SCHEMA_DIR}/acn_attributes.toml` (frozen set, CON-7). Semantic conventions pinned at **{}** (TRC-2). {} spans, {} events, {} attributes ({} promoted to typed columns, TRC-25), {} run options, {} provider mappings.\n\n",
        inv.semconv_version(),
        inv.spans().len(),
        inv.events().len(),
        inv.attributes().len(),
        inv.promoted().count(),
        inv.options().len(),
        inv.providers().len(),
    ));
    o.push_str("An optional attribute is present when its condition holds and absent, never zero or empty, otherwise.\n\n");
    o.push_str("## Spans\n\n| Span | Kind | Parent | Producers | Spec |\n|---|---|---|---|---|\n");
    for s in inv.spans() {
        o.push_str(&format!(
            "| `{}` | {} | {} | {} | {} |\n",
            s.name,
            s.kind
                .iter()
                .map(|k| k.as_str())
                .collect::<Vec<_>>()
                .join(" or "),
            s.parents
                .iter()
                .map(|p| format!("`{p}`"))
                .collect::<Vec<_>>()
                .join(", "),
            s.producers.join(", "),
            s.requirement
        ));
    }
    o.push_str(
        "\n## Span events\n\n| Event | On | Fields | Producers | Spec |\n|---|---|---|---|---|\n",
    );
    for e in inv.events() {
        let fields: Vec<String> = e
            .fields
            .iter()
            .map(|f| {
                let mut s = format!("`{}` {}", f.name, ty(f.ty));
                if !f.unit.is_empty() {
                    s.push(' ');
                    s.push_str(&f.unit);
                }
                if !f.values.is_empty() {
                    s.push_str(&format!(" ({})", f.values.join(" \\| ")));
                }
                s
            })
            .collect();
        o.push_str(&format!(
            "| `{}` | `{}` | {} | {} | {} |\n",
            e.name,
            e.on,
            cell(&fields.join(", ")),
            e.producers.join(", "),
            e.requirement
        ));
    }
    o.push_str("\n## Attributes\n\n| Attribute | Type | Unit | On | Present | Promoted | Values | Producers | Spec | Meaning |\n|---|---|---|---|---|---|---|---|---|---|\n");
    for a in inv.attributes() {
        let on: Vec<String> = a.on.iter().map(|s| format!("`{s}`")).collect();
        let values: Vec<String> = a.values.iter().map(|v| format!("`{v}`")).collect();
        o.push_str(&format!(
            "| `{}` | {} | {} | {} | {} | {} | {} | {} | {} | {} |\n",
            a.name,
            ty(a.ty),
            cell(&a.unit),
            on.join(", "),
            a.when
                .as_deref()
                .map_or_else(|| "always".to_owned(), |w| format!("when {w}")),
            if a.promoted { "yes" } else { "no" },
            cell(&values.join(", ")),
            a.producers.join(", "),
            a.requirement,
            cell(&a.doc),
        ));
    }
    o.push_str("\n## Run options (CON-29)\n\nAn option enters `params_hash` only when it differs from its default.\n\n| Option | Type | Default | Recorded as | Meaning |\n|---|---|---|---|---|\n");
    for opt in inv.options() {
        o.push_str(&format!(
            "| `{}` | {} | `{}` | `{}` | {} |\n",
            opt.name,
            ty(opt.ty),
            opt.default,
            opt.attribute,
            cell(&opt.doc)
        ));
    }
    o.push_str("\n## Provider normalisation (TRC-21)\n\nPaths address the complete response object; for a streamed call, what the stream assembles to. The input token total is the sum of the listed fields; without the base field there is no usage and every count is absent. A missing cache-read field means zero only where the provider always reports it. A stop value that is not listed maps to `other`; the raw value is always kept; no provider value maps to `client_abort` or `transport_error`.\n\n| Provider | Input tokens (total) | Base | Output tokens | Cache read | If absent | Cache write | Stop field | Stop values |\n|---|---|---|---|---|---|---|---|---|\n");
    for p in inv.providers() {
        let inputs: Vec<String> = p.input_tokens.iter().map(|f| format!("`{f}`")).collect();
        let stops: Vec<String> = p
            .stop_reason
            .iter()
            .map(|(k, v)| format!("`{k}` → `{v}`"))
            .collect();
        o.push_str(&format!(
            "| `{}` | {} | `{}` | `{}` | `{}` | {} | {} | `{}` | {} |\n",
            p.name,
            inputs.join(" + "),
            p.input_tokens_base,
            p.output_tokens,
            p.cache_read,
            p.cache_read_absent.as_str(),
            p.cache_write
                .as_deref()
                .map_or_else(|| "none (0)".to_owned(), |w| format!("`{w}`")),
            p.stop_reason_field,
            stops.join(", ")
        ));
    }
    o
}
