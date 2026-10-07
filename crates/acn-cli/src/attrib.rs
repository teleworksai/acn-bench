//! `acn attrib` (SPEC 090 §5): a bundle's per-turn attribution as Parquet, and
//! a verdict's cells as a heatmap. Nothing here computes a quantity: the
//! numbers are `acn-attrib`'s, through `acn-hyp` (ATR-31).

use std::collections::BTreeMap;
use std::path::Path;
use std::sync::Arc;

use acn_attrib::core::Decomposition;
use acn_hyp::quantities::{QUANTITIES, Replicate, attribution, attribution_value};
use acn_hyp::read::BundleData;
use arrow_array::builder::{FixedSizeBinaryBuilder, Int64Builder, MapBuilder, StringBuilder};
use arrow_array::{ArrayRef, RecordBatch};
use arrow_schema::{DataType, Field, Fields, Schema};
use plotters::prelude::*;
use serde_json::value::RawValue;
use serde_json::{Value, json};

/// The file `acn attrib turns` writes (ATR-40).
pub const TURNS_FILE: &str = "attribution.parquet";

/// The labels a bundle's numbers carry (CON-26, HYP-23): a mock bundle is
/// mock-gated, a `sim` bundle sim-only, and one whose hypothesis is not frozen
/// exploratory, as a verdict would label them.
#[must_use]
pub fn labels(b: &BundleData) -> Vec<&'static str> {
    let m = &b.manifest;
    let mut out = Vec::new();
    if m.hypothesis.status != "frozen" {
        out.push("exploratory");
    }
    if m.backend == "mockllm" {
        out.push("mock-gated");
    }
    if m.mode == "sim" {
        out.push("sim-only");
    }
    out
}

/// The schema of `attribution.parquet` (ATR-40).
#[must_use]
pub fn turns_schema() -> Schema {
    let int = |n: &str| Field::new(n, DataType::Int64, false);
    let hop = DataType::Map(
        Arc::new(Field::new(
            "entries",
            DataType::Struct(Fields::from(vec![
                Field::new("key", DataType::Utf8, false),
                Field::new("value", DataType::Int64, true),
            ])),
            false,
        )),
        false,
    );
    Schema::new(vec![
        Field::new("run_id", DataType::Utf8, false),
        Field::new("role", DataType::Utf8, false),
        int("replicate"),
        Field::new("session_id", DataType::FixedSizeBinary(8), false),
        int("turn_index"),
        int("duration_ns"),
        int("network_ns"),
        int("model_ns"),
        int("tool_ns"),
        int("retry_ns"),
        int("other_ns"),
        Field::new("queue_wait_ns", DataType::Int64, true),
        int("stalls"),
        int("retries"),
        int("clipped_ns"),
        int("unattributed_link_ns"),
        Field::new("hop_ns", hop, false),
    ])
}

fn batch(run_id: &str, rows: &[(&str, i64, &Decomposition)]) -> Result<RecordBatch, String> {
    let mut run = StringBuilder::new();
    let mut role = StringBuilder::new();
    let mut session = FixedSizeBinaryBuilder::new(8);
    let mut ints: Vec<Int64Builder> = (0..13).map(|_| Int64Builder::new()).collect();
    let mut hops = MapBuilder::new(None, StringBuilder::new(), Int64Builder::new());
    for (r, rep, d) in rows {
        run.append_value(run_id);
        role.append_value(r);
        session
            .append_value(d.session_id)
            .map_err(|e| e.to_string())?;
        let p = d.parts;
        let values = [
            Some(*rep),
            Some(d.turn_index),
            Some(p.duration_ns),
            Some(p.network_ns),
            Some(p.model_ns),
            Some(p.tool_ns),
            Some(p.retry_ns),
            Some(p.other_ns),
            d.queue_wait_ns,
            Some(d.stalls),
            Some(d.retries),
            Some(d.clipped_ns),
            Some(d.unattributed_link_ns),
        ];
        for (b, v) in ints.iter_mut().zip(values) {
            b.append_option(v);
        }
        for (h, ns) in &d.hop_ns {
            hops.keys().append_value(h.key());
            hops.values().append_value(*ns);
        }
        hops.append(true).map_err(|e| e.to_string())?;
    }
    let mut cols: Vec<ArrayRef> = vec![Arc::new(run.finish()), Arc::new(role.finish())];
    let mut ints = ints.into_iter();
    let mut next = || ints.next().map(|mut b| Arc::new(b.finish()) as ArrayRef);
    cols.push(next().ok_or("columns")?);
    cols.push(Arc::new(session.finish()));
    for _ in 0..12 {
        cols.push(next().ok_or("columns")?);
    }
    cols.push(Arc::new(hops.finish()));
    RecordBatch::try_new(Arc::new(turns_schema()), cols).map_err(|e| e.to_string())
}

/// `acn attrib turns <bundle> --out <dir>` (ATR-40): one object, `ok: false`
/// with the reason when the bundle does not verify or its attribution fails.
#[must_use]
pub fn turns(bundle: &Path, out: &Path) -> Value {
    match turns_inner(bundle, out) {
        Ok(v) => v,
        Err(e) => json!({"ok": false, "error": e}),
    }
}

/// Write `bytes` to `dir/name` through a fresh temporary file renamed into
/// place: a link already at `dir/name` is replaced, never written through, so
/// nothing outside `dir` changes (TRC-23).
fn write_beside(dir: &Path, name: &str, bytes: &[u8]) -> Result<std::path::PathBuf, String> {
    use std::io::Write as _;
    std::fs::create_dir_all(dir).map_err(|e| format!("{}: {e}", dir.display()))?;
    let file = dir.join(name);
    let tmp = dir.join(format!(".{name}.tmp"));
    let _ = std::fs::remove_file(&tmp);
    let mut f = std::fs::OpenOptions::new()
        .write(true)
        .create_new(true)
        .open(&tmp)
        .map_err(|e| format!("{}: {e}", tmp.display()))?;
    f.write_all(bytes)
        .and_then(|()| f.sync_all())
        .map_err(|e| format!("{}: {e}", tmp.display()))?;
    std::fs::rename(&tmp, &file).map_err(|e| format!("{}: {e}", file.display()))?;
    Ok(file)
}

/// `path` with links and `..` resolved as far as it exists, and the rest
/// appended: where a write to it would land.
fn resolved(path: &Path) -> Result<std::path::PathBuf, String> {
    let abs = std::path::absolute(path).map_err(|e| format!("{}: {e}", path.display()))?;
    let mut rest = Vec::new();
    let mut at = abs.as_path();
    loop {
        if let Ok(c) = std::fs::canonicalize(at) {
            let mut out = c;
            for part in rest.iter().rev() {
                out.push(part);
            }
            return Ok(out);
        }
        match (at.parent(), at.file_name()) {
            (Some(p), Some(name)) => {
                rest.push(name.to_owned());
                at = p;
            }
            _ => return Err(format!("{}: no existing ancestor", path.display())),
        }
    }
}

fn turns_inner(bundle: &Path, out: &Path) -> Result<Value, String> {
    let b = acn_hyp::read::read(bundle).map_err(|e| e.to_string())?;
    // A bundle is immutable (TRC-23): nothing is written into it.
    if resolved(out)?.starts_with(resolved(bundle)?) {
        return Err(format!(
            "--out {} is inside the bundle, which is immutable (TRC-23, ATR-40)",
            out.display()
        ));
    }
    let ds = b
        .attrib
        .as_ref()
        .map_err(|e| format!("{}: {e} (SPEC 090 ATR-15)", bundle.display()))?;
    let who: BTreeMap<[u8; 8], (&str, i64)> = b
        .sessions
        .iter()
        .map(|s| (s.session_id, (s.role.as_str(), s.replicate)))
        .collect();
    let mut rows = Vec::with_capacity(ds.len());
    let mut groups: BTreeMap<(&str, i64), Replicate> = BTreeMap::new();
    for d in ds {
        let &(role, rep) = who
            .get(&d.session_id)
            .ok_or("a turn of a session the session view does not hold")?;
        rows.push((role, rep, d));
        groups.entry((role, rep)).or_default().parts.push(d.parts);
    }
    let rb = batch(&b.manifest.run_id, &rows)?;
    let bytes = acn_trace::parquet_io::encode(&rb).map_err(|e| e.to_string())?;
    let mut replicates = Vec::new();
    for ((role, rep), r) in &groups {
        let mut o = serde_json::Map::new();
        o.insert("role".into(), json!(role));
        o.insert("replicate".into(), json!(rep));
        for q in QUANTITIES.iter().filter(|q| attribution(q.name).is_some()) {
            let v =
                attribution_value(q.name, r).map_err(|e| format!("{}: {e}", bundle.display()))?;
            o.insert(q.name.into(), v.map_or(Value::Null, |x| json!(x)));
        }
        replicates.push(Value::Object(o));
    }
    let file = write_beside(out, TURNS_FILE, &bytes)?;
    Ok(json!({
        "ok": true,
        "run_id": b.manifest.run_id,
        "mode": b.manifest.mode,
        "backend": b.manifest.backend,
        "labels": labels(&b),
        "file": file.display().to_string(),
        "blake3": blake3::hash(&bytes).to_hex().to_string(),
        "rows": rows.len(),
        "replicates": replicates,
    }))
}

/// What a heatmap draws (ATR-41).
pub struct Heatmap<'a> {
    pub verdict: &'a Path,
    pub slice: &'a str,
    pub quantity: &'a str,
    pub x: &'a str,
    pub y: &'a str,
    pub out: &'a Path,
}

/// The parts of `verdict.json` a heatmap reads (HYP-15). Numbers are kept as
/// the verdict wrote them, so a cell shows the verdict's own text (CON-27(c)).
#[derive(serde::Deserialize)]
struct VerdictDoc {
    format: String,
    labels: Vec<String>,
    slices: Vec<SliceDoc>,
}

#[derive(serde::Deserialize)]
struct SliceDoc {
    key: String,
    labels: Vec<String>,
    cells: Vec<CellDoc>,
}

#[derive(serde::Deserialize)]
struct CellDoc {
    params: BTreeMap<String, Box<RawValue>>,
    effects: Option<BTreeMap<String, EffectDoc>>,
}

#[derive(serde::Deserialize)]
struct EffectDoc {
    value: Box<RawValue>,
    ci_low: Box<RawValue>,
    ci_high: Box<RawValue>,
}

/// A parameter value as text: a string as itself, anything else as written.
fn text(v: &RawValue) -> String {
    serde_json::from_str::<String>(v.get()).unwrap_or_else(|_| v.get().to_owned())
}

/// A number as the verdict wrote it, and its value; `None` for `null`.
fn number(v: &RawValue) -> Result<Option<(f64, String)>, String> {
    let t = v.get();
    if t == "null" {
        return Ok(None);
    }
    t.parse::<f64>()
        .map(|x| Some((x, t.to_owned())))
        .map_err(|_| format!("`{t}` is not a number"))
}

/// `acn attrib heatmap` (ATR-41): one object, `ok: false` with the reason.
#[must_use]
pub fn heatmap(h: &Heatmap<'_>) -> Value {
    match heatmap_inner(h) {
        Ok(v) => v,
        Err(e) => json!({"ok": false, "error": e}),
    }
}

/// One cell: its x and y values, and the effect with its interval if any.
type Cell = (String, String, Option<(f64, String, String, String)>);

fn heatmap_inner(h: &Heatmap<'_>) -> Result<Value, String> {
    if h.x == h.y {
        return Err("--x and --y name the same parameter (ATR-41)".into());
    }
    let raw =
        std::fs::read_to_string(h.verdict).map_err(|e| format!("{}: {e}", h.verdict.display()))?;
    let v: VerdictDoc = serde_json::from_str(&raw).map_err(|e| {
        format!(
            "{}: not a verdict.json with its labels and slices (HYP-15): {e}",
            h.verdict.display()
        )
    })?;
    let s = v
        .slices
        .iter()
        .find(|s| s.key == h.slice)
        .ok_or_else(|| format!("the verdict has no slice `{}`", h.slice))?;
    if !s.cells.iter().any(|c| {
        c.effects
            .as_ref()
            .is_some_and(|e| e.contains_key(h.quantity))
    }) {
        return Err(format!(
            "the slice records no effect of `{}`: a verdict records effects of its primary measures (ATR-41)",
            h.quantity
        ));
    }
    // A parameter other than x and y may appear only with one value across the
    // slice (a non-pooled parameter, or a single level): it does not vary.
    let mut others: BTreeMap<&str, &str> = BTreeMap::new();
    let mut grid: Vec<Cell> = Vec::new();
    for c in &s.cells {
        for (k, val) in &c.params {
            if k == h.x || k == h.y {
                continue;
            }
            match others.get(k.as_str()) {
                None => {
                    others.insert(k, val.get());
                }
                Some(first) if *first == val.get() => {}
                Some(_) => {
                    return Err(format!(
                        "the slice varies `{k}` besides `{}` and `{}` (ATR-41)",
                        h.x, h.y
                    ));
                }
            }
        }
        let at = |p: &str| {
            c.params
                .get(p)
                .map(|v| text(v))
                .ok_or_else(|| format!("a cell does not set `{p}` (ATR-41)"))
        };
        let (x, y) = (at(h.x)?, at(h.y)?);
        if grid.iter().any(|g| g.0 == x && g.1 == y) {
            return Err(format!(
                "two cells share {}={x}, {}={y}: the slice varies another parameter (ATR-41)",
                h.x, h.y
            ));
        }
        let effect = match c.effects.as_ref().and_then(|e| e.get(h.quantity)) {
            None => None,
            Some(e) => match number(&e.value)? {
                None => None,
                Some((val, t)) => {
                    let bound = |b: &RawValue| -> Result<String, String> {
                        Ok(number(b)?.map_or_else(|| "null".to_owned(), |(_, t)| t))
                    };
                    Some((val, t, bound(&e.ci_low)?, bound(&e.ci_high)?))
                }
            },
        };
        grid.push((x, y, effect));
    }
    let order = |f: &dyn Fn(&Cell) -> &String| {
        let mut seen: Vec<String> = Vec::new();
        for c in &grid {
            if !seen.contains(f(c)) {
                seen.push(f(c).clone());
            }
        }
        seen
    };
    let xs = order(&|c| &c.0);
    let ys = order(&|c| &c.1);
    let mut labels: Vec<String> = Vec::new();
    for l in v.labels.iter().chain(&s.labels) {
        if !labels.contains(l) {
            labels.push(l.clone());
        }
    }
    let title = format!(
        "{}: effect by {} and {}, slice {}; labels: {}",
        h.quantity,
        h.x,
        h.y,
        h.slice,
        if labels.is_empty() {
            "none".to_owned()
        } else {
            labels.join(", ")
        }
    );
    let svg = draw(&title, &xs, &ys, &grid, h)?;
    if let Some(dir) = h.out.parent().filter(|d| !d.as_os_str().is_empty()) {
        std::fs::create_dir_all(dir).map_err(|e| format!("{}: {e}", dir.display()))?;
    }
    std::fs::write(h.out, &svg).map_err(|e| format!("{}: {e}", h.out.display()))?;
    Ok(json!({
        "ok": true,
        "format": v.format,
        "file": h.out.display().to_string(),
        "blake3": blake3::hash(svg.as_bytes()).to_hex().to_string(),
        "cells": grid.len(),
        "labels": labels,
    }))
}

const CELL_W: i32 = 180;
const CELL_H: i32 = 60;
const LEFT: i32 = 140;
const TOP: i32 = 80;

/// The heatmap as SVG: white for no effect, red for a positive one and blue for
/// a negative one, scaled by the largest magnitude; an empty cell is grey and
/// marked. Text is written as SVG text, so no font is read (ATR-41).
fn draw(
    title: &str,
    xs: &[String],
    ys: &[String],
    grid: &[Cell],
    h: &Heatmap<'_>,
) -> Result<String, String> {
    let w = u32::try_from(LEFT + CELL_W * i32::try_from(xs.len()).map_err(|e| e.to_string())? + 20)
        .map_err(|e| e.to_string())?;
    let ht = u32::try_from(TOP + CELL_H * i32::try_from(ys.len()).map_err(|e| e.to_string())? + 20)
        .map_err(|e| e.to_string())?;
    let scale = grid
        .iter()
        .filter_map(|c| c.2.as_ref().map(|e| e.0.abs()))
        .fold(0.0_f64, f64::max);
    let mut svg = String::new();
    {
        let root = SVGBackend::with_string(&mut svg, (w, ht)).into_drawing_area();
        let e = |x: DrawingAreaErrorKind<_>| x.to_string();
        root.fill(&WHITE).map_err(e)?;
        let font = ("sans-serif", 14).into_font();
        let small = ("sans-serif", 12).into_font();
        root.draw(&Text::new(title.to_owned(), (10, 10), font.clone()))
            .map_err(e)?;
        root.draw(&Text::new(
            format!("x: {}, y: {}", h.x, h.y),
            (10, 34),
            small.clone(),
        ))
        .map_err(e)?;
        for (i, x) in (0..).zip(xs) {
            root.draw(&Text::new(
                x.clone(),
                (LEFT + i * CELL_W + 8, TOP - 20),
                small.clone(),
            ))
            .map_err(e)?;
        }
        for (j, y) in (0..).zip(ys) {
            root.draw(&Text::new(
                y.clone(),
                (10, TOP + j * CELL_H + 20),
                small.clone(),
            ))
            .map_err(e)?;
        }
        for c in grid {
            let i = (0..)
                .zip(xs)
                .find(|(_, x)| **x == c.0)
                .map_or(0, |(i, _)| i);
            let j = (0..)
                .zip(ys)
                .find(|(_, y)| **y == c.1)
                .map_or(0, |(j, _)| j);
            let (x0, y0) = (LEFT + i * CELL_W, TOP + j * CELL_H);
            let (fill, lines) = match &c.2 {
                Some((v, t, lo, hi)) => {
                    let k = if scale > 0.0 {
                        (v.abs() / scale).min(1.0)
                    } else {
                        0.0
                    };
                    // Truncation to a channel value is the colour scale's own rounding.
                    #[allow(clippy::cast_possible_truncation, clippy::cast_sign_loss)]
                    let fade = (255.0 * (1.0 - k)).round() as u8;
                    let colour = if *v >= 0.0 {
                        RGBColor(255, fade, fade)
                    } else {
                        RGBColor(fade, fade, 255)
                    };
                    (colour, vec![t.clone(), format!("[{lo}, {hi}]")])
                }
                None => (RGBColor(220, 220, 220), vec!["no value".to_owned()]),
            };
            root.draw(&Rectangle::new(
                [(x0, y0), (x0 + CELL_W - 4, y0 + CELL_H - 4)],
                fill.filled(),
            ))
            .map_err(e)?;
            for (k, line) in (0..).zip(lines) {
                root.draw(&Text::new(line, (x0 + 8, y0 + 8 + 20 * k), small.clone()))
                    .map_err(e)?;
            }
        }
        root.present().map_err(e)?;
    }
    Ok(svg)
}
