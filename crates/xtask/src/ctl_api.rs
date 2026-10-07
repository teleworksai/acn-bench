//! CTL-24: the control plane's API page, rendered from the committed OpenAPI
//! document. It reads only the JSON, so `xtask` does not depend on `acn-ctl`.

use std::fmt::Write as _;
use std::path::Path;

use serde_json::Value;

use crate::workspace::read;
use crate::{Error, Result};

/// The document, relative to the root.
pub const OPENAPI_FILE: &str = "crates/acn-ctl/openapi.json";

const METHODS: [&str; 4] = ["get", "post", "put", "delete"];

fn cell(s: &str) -> String {
    s.replace('|', "\\|").replace('\n', " ")
}

fn invalid(what: &str) -> Error {
    Error::Invalid(format!("{OPENAPI_FILE}: {what} (CTL-24)"))
}

/// A schema as one cell: its `$ref` name, else its type.
fn schema_name(s: &Value) -> String {
    if let Some(r) = s.get("$ref").and_then(Value::as_str) {
        let name = r.rsplit('/').next().unwrap_or(r);
        return format!("[`{name}`](#{})", name.to_ascii_lowercase());
    }
    match (
        s.get("type").and_then(Value::as_str),
        s.get("format").and_then(Value::as_str),
    ) {
        (Some(t), Some(f)) => format!("{t} ({f})"),
        (Some(t), None) => t.to_owned(),
        _ => "value".to_owned(),
    }
}

/// The media type and schema of a `content` map, as one cell.
fn content(c: Option<&Value>) -> String {
    let Some(map) = c.and_then(Value::as_object) else {
        return String::new();
    };
    map.iter()
        .map(|(ct, v)| match v.get("schema") {
            Some(sc) => format!("`{ct}` {}", schema_name(sc)),
            None => format!("`{ct}`"),
        })
        .collect::<Vec<_>>()
        .join(", ")
}

fn property_type(p: &Value) -> String {
    if let Some(e) = p.get("enum").and_then(Value::as_array) {
        let vals: Vec<String> = e
            .iter()
            .map(|v| format!("`{}`", cell(&v.to_string())))
            .collect();
        return vals.join(" \\| ");
    }
    if let Some(c) = p.get("const") {
        return format!("`{}`", cell(&c.to_string()));
    }
    if p.get("oneOf").is_some() {
        return "one of the forms below".to_owned();
    }
    if p.get("type").and_then(Value::as_str) == Some("array") {
        return format!(
            "array of {}",
            p.get("items").map(property_type).unwrap_or_default()
        );
    }
    cell(&schema_name(p))
}

/// A property's constraints and description, as one cell.
fn notes(p: &Value) -> String {
    let mut n = Vec::new();
    if let Some(v) = p.get("pattern").and_then(Value::as_str) {
        n.push(format!("pattern `{v}`"));
    }
    if let Some(v) = p.get("minimum") {
        n.push(format!("minimum {v}"));
    }
    if let Some(v) = p.get("description").and_then(Value::as_str) {
        n.push(v.to_owned());
    }
    cell(&n.join("; "))
}

/// The rows of an object's fields, nested objects and forms flattened
/// under a dotted prefix.
fn rows(s: &mut String, prefix: &str, sc: &Value) {
    let required: Vec<&str> = sc
        .get("required")
        .and_then(Value::as_array)
        .map(|r| r.iter().filter_map(Value::as_str).collect())
        .unwrap_or_default();
    let Some(props) = sc.get("properties").and_then(Value::as_object) else {
        return;
    };
    for (k, p) in props {
        let name = format!("{prefix}{k}");
        let _ = writeln!(
            s,
            "| `{}` | {} | {} | {} |",
            cell(&name),
            property_type(p),
            if required.contains(&k.as_str()) {
                "yes"
            } else {
                ""
            },
            notes(p)
        );
        rows(s, &format!("{name}."), p);
        if let Some(forms) = p.get("oneOf").and_then(Value::as_array) {
            for (i, f) in forms.iter().enumerate() {
                rows(s, &format!("{name} (form {}).", i + 1), f);
            }
        }
    }
}

/// The page, without the generated-file header.
pub fn page(doc: &Value) -> Result<String> {
    let paths = doc
        .get("paths")
        .and_then(Value::as_object)
        .ok_or_else(|| invalid("no `paths` object"))?;
    let title = doc["info"]["title"].as_str().unwrap_or("API");
    let version = doc["openapi"].as_str().unwrap_or("?");
    let mut s = String::new();
    let _ = writeln!(s, "# `{title}` API\n");
    let _ = writeln!(
        s,
        "The control plane's HTTP/JSON API (SPEC 070), rendered from `{OPENAPI_FILE}` (OpenAPI {version}). The server's router and that document are built from one route table (CTL-24).\n"
    );
    let _ = writeln!(s, "## Routes\n");
    let _ = writeln!(
        s,
        "| Method | Path | Operation | Summary | Body | Statuses |"
    );
    let _ = writeln!(s, "|---|---|---|---|---|---|");
    for (path, ops) in paths {
        let ops = ops
            .as_object()
            .ok_or_else(|| invalid(&format!("`{path}` is not an object")))?;
        if let Some(m) = ops.keys().find(|m| !METHODS.contains(&m.as_str())) {
            return Err(invalid(&format!(
                "`{path}` has `{m}`, which this page does not render"
            )));
        }
        for m in METHODS {
            let Some(op) = ops.get(m) else { continue };
            let codes: Vec<&str> = op
                .get("responses")
                .and_then(Value::as_object)
                .map(|r| r.keys().map(String::as_str).collect())
                .unwrap_or_default();
            let body = op
                .get("requestBody")
                .map(|b| {
                    let c = content(b.get("content"));
                    if b.get("required").and_then(Value::as_bool) == Some(true) {
                        c
                    } else {
                        format!("{c} (optional)")
                    }
                })
                .unwrap_or_default();
            let _ = writeln!(
                s,
                "| {} | `{}` | `{}` | {} | {} | {} |",
                m.to_ascii_uppercase(),
                cell(path),
                cell(op["operationId"].as_str().unwrap_or("")),
                cell(op["summary"].as_str().unwrap_or("")),
                cell(&body),
                codes.join(", ")
            );
        }
    }
    let _ = writeln!(s, "\n## Responses\n");
    for (path, ops) in paths {
        let Some(ops) = ops.as_object() else { continue };
        for m in METHODS {
            let Some(op) = ops.get(m) else { continue };
            let _ = writeln!(s, "### {} `{}`\n", m.to_ascii_uppercase(), cell(path));
            let _ = writeln!(s, "| Status | Meaning | Content |");
            let _ = writeln!(s, "|---|---|---|");
            if let Some(r) = op.get("responses").and_then(Value::as_object) {
                for (code, v) in r {
                    let _ = writeln!(
                        s,
                        "| {code} | {} | {} |",
                        cell(v["description"].as_str().unwrap_or("")),
                        cell(&content(v.get("content")))
                    );
                }
            }
            let _ = writeln!(s);
        }
    }
    let _ = writeln!(s, "## Schemas\n");
    if let Some(schemas) = doc["components"]["schemas"].as_object() {
        for (name, sc) in schemas {
            let _ = writeln!(s, "### {name}\n");
            if let Some(d) = sc.get("description").and_then(Value::as_str) {
                let _ = writeln!(s, "{d}\n");
            }
            let closed = sc.get("additionalProperties") == Some(&Value::Bool(false));
            if sc
                .get("properties")
                .and_then(Value::as_object)
                .is_some_and(|p| !p.is_empty())
            {
                let _ = writeln!(s, "| Field | Type | Required | Notes |");
                let _ = writeln!(s, "|---|---|---|---|");
                rows(&mut s, "", sc);
                let _ = writeln!(s);
            } else if closed {
                let _ = writeln!(s, "The empty object.\n");
            } else {
                let _ = writeln!(s, "An object.\n");
            }
            if closed {
                let _ = writeln!(s, "Other fields are refused.\n");
            }
        }
    }
    Ok(s.trim_end().to_owned() + "\n")
}

/// The page, when the root has the document.
pub fn load(root: &Path) -> Result<Option<String>> {
    let path = root.join(OPENAPI_FILE);
    if !path.is_file() {
        return Ok(None);
    }
    let doc: Value = serde_json::from_str(&read(&path)?)?;
    page(&doc).map(Some)
}
