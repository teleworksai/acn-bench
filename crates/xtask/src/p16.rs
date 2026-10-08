//! `cargo xtask p16-compare` (SPEC 140 P16-12): compare one run's bundles,
//! made on different targets, in build-neutral form. A difference is evidence
//! for SPEC 140's first open question, never a failure (CON-31).

use std::collections::{BTreeMap, BTreeSet};
use std::path::Path;

use serde::Serialize;
use serde_json::Value;

use crate::workspace::read;
use crate::{Error, Result};

/// The comparison (CON-8).
#[derive(Debug, Serialize)]
pub struct Report {
    pub ok: bool,
    /// Each record's target, in the order given.
    pub targets: Vec<String>,
    pub run_id: String,
    /// Whether every expected target left a record and every record agrees
    /// with the first.
    pub identical: bool,
    /// How many records were expected, and how many arrived.
    pub expected: usize,
    pub records: usize,
    /// Per target that differs from the first: the files, and `resources` or
    /// `manifest` keys, that differ, in bytewise order.
    pub differ: BTreeMap<String, Vec<String>>,
}

fn field<'v>(v: &'v Value, k: &str, file: &Path) -> Result<&'v Value> {
    v.get(k)
        .ok_or_else(|| Error::Invalid(format!("{}: no `{k}` (P16-12)", file.display())))
}

fn text<'v>(v: &'v Value, k: &str, file: &Path) -> Result<&'v str> {
    field(v, k, file)?
        .as_str()
        .filter(|s| !s.is_empty())
        .ok_or_else(|| {
            Error::Invalid(format!(
                "{}: `{k}` is not a non-empty string (P16-12)",
                file.display()
            ))
        })
}

/// The keys of two objects whose values differ, `prefix`ed.
fn differing(a: &Value, b: &Value, prefix: &str, out: &mut Vec<String>) {
    let keys: BTreeSet<&String> = a
        .as_object()
        .into_iter()
        .chain(b.as_object())
        .flat_map(|o| o.keys())
        .collect();
    for k in keys {
        if a.get(k) != b.get(k) {
            out.push(format!("{prefix}{k}"));
        }
    }
}

/// Compare the `acn bundle neutral` records in `files`, `expect` of them
/// expected. Fewer is reported, not refused, so a target whose job failed
/// still leaves the others' comparison.
pub fn run(files: &[impl AsRef<Path>], expect: usize) -> Result<Report> {
    let mut records = Vec::new();
    for f in files {
        let f = f.as_ref();
        let v: Value = serde_json::from_str(&read(f)?)?;
        if v.get("ok") != Some(&Value::Bool(true)) {
            return Err(Error::Invalid(format!(
                "{}: a record that is not a verified bundle's: {v}",
                f.display()
            )));
        }
        records.push((f.to_path_buf(), v));
    }
    let Some((first_file, first)) = records.first() else {
        return Err(Error::Invalid("no records to compare (P16-12)".into()));
    };
    let run_id = text(first, "run_id", first_file)?.to_owned();
    let mut targets: Vec<String> = Vec::new();
    let mut differ = BTreeMap::new();
    for (file, v) in &records {
        let target = text(v, "target", file)?.to_owned();
        if targets.contains(&target) {
            return Err(Error::Invalid(format!(
                "{}: a second record of target `{target}` (P16-12)",
                file.display()
            )));
        }
        targets.push(target.clone());
        if text(v, "run_id", file)? != run_id {
            return Err(Error::Invalid(format!(
                "{}: another run than {run_id}: the records must be of one run (P16-12)",
                file.display()
            )));
        }
        let mut d: Vec<String> = Vec::new();
        differing(
            field(first, "files", first_file)?,
            field(v, "files", file)?,
            "",
            &mut d,
        );
        if field(first, "resources", first_file)? != field(v, "resources", file)? {
            d.push("resources".into());
        }
        differing(
            field(first, "manifest", first_file)?,
            field(v, "manifest", file)?,
            "manifest.",
            &mut d,
        );
        d.sort();
        if !d.is_empty() {
            differ.insert(target, d);
        }
    }
    Ok(Report {
        ok: true,
        identical: differ.is_empty() && records.len() >= expect,
        expected: expect,
        records: records.len(),
        targets,
        run_id,
        differ,
    })
}
