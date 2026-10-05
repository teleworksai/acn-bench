//! Measured impairment traces (SPEC 020 §6, EMU-60 to EMU-64; CON-21): a
//! directory `scenarios/measured/<slug>/` holding `trace.toml`, a series of
//! link-state samples, and `provenance.toml`, where the series came from.
//! Loading checks both, so that a trace without its provenance, or with a field
//! that could carry an identifier, never enters a scenario.

use std::path::Path;

use serde::Deserialize;

/// The licences that allow redistribution, by SPDX identifier (EMU-61).
pub const REDISTRIBUTABLE: &[&str] = &[
    "CC-BY-4.0",
    "CC-BY-SA-4.0",
    "CC-BY-3.0",
    "CC0-1.0",
    "ODbL-1.0",
    "CDLA-Permissive-2.0",
    "MIT",
    "Apache-2.0",
];

/// Why a trace was refused.
#[derive(Debug, Clone, PartialEq, Eq, thiserror::Error)]
#[error("{reason}: {message}")]
pub struct TraceError {
    /// `layout`, `parse`, `samples`, `range`, `licence` or `provenance`.
    pub reason: &'static str,
    pub message: String,
}

fn refuse<T>(reason: &'static str, message: impl Into<String>) -> Result<T, TraceError> {
    Err(TraceError {
        reason,
        message: message.into(),
    })
}

/// A sample's round-trip times, in milliseconds.
#[derive(Debug, Clone, Copy, PartialEq, Deserialize)]
#[serde(deny_unknown_fields)]
pub struct Rtt {
    pub avg: f64,
    pub min: f64,
    pub max: f64,
    pub stdev: f64,
}

/// One link-state sample (EMU-62): nothing that could identify a place, a
/// device or a person (EMU-63).
#[derive(Debug, Clone, Copy, PartialEq, Deserialize)]
#[serde(deny_unknown_fields)]
pub struct Sample {
    pub t_s: f64,
    pub loss: f64,
    /// Absent exactly when `loss` is 1: no probe returned.
    #[serde(default)]
    pub rtt_ms: Option<Rtt>,
    pub dl_kbps: f64,
    pub ul_kbps: f64,
}

/// `trace.toml` (EMU-62).
#[derive(Debug, Clone, PartialEq, Deserialize)]
#[serde(deny_unknown_fields)]
pub struct Trace {
    pub schema_version: u32,
    pub sample_interval_s: f64,
    #[serde(rename = "sample")]
    pub samples: Vec<Sample>,
}

/// A source file the conversion read.
#[derive(Debug, Clone, PartialEq, Eq, Deserialize)]
#[serde(deny_unknown_fields)]
pub struct SourceFile {
    pub name: String,
    pub blake3: String,
}

#[derive(Debug, Clone, PartialEq, Eq, Deserialize)]
#[serde(deny_unknown_fields)]
pub struct Source {
    pub title: String,
    pub url: String,
    #[serde(default)]
    pub identifier: Option<String>,
}

#[derive(Debug, Clone, PartialEq, Eq, Deserialize)]
#[serde(deny_unknown_fields)]
pub struct Licence {
    pub spdx: String,
    pub attribution: String,
}

/// How the source was measured (EMU-61).
#[derive(Debug, Clone, PartialEq, Eq, Deserialize)]
#[serde(deny_unknown_fields)]
pub struct Collection {
    pub date: String,
    pub device: String,
    pub technology: String,
    /// `testbed` or `commercial`.
    pub network: String,
    pub location_class: String,
    /// `static`, `walk` or `drive`.
    pub mobility: String,
    pub method: String,
}

/// How the source became `trace.toml` (EMU-61).
#[derive(Debug, Clone, PartialEq, Eq, Deserialize)]
#[serde(deny_unknown_fields)]
pub struct Conversion {
    pub tool: String,
    pub tool_path: String,
    pub tool_blake3: String,
    pub sources: Vec<SourceFile>,
    pub dropped: Vec<String>,
}

/// `provenance.toml` (EMU-61, CON-21).
#[derive(Debug, Clone, PartialEq, Eq, Deserialize)]
#[serde(deny_unknown_fields)]
pub struct Provenance {
    pub source: Source,
    pub licence: Licence,
    pub collection: Collection,
    pub conversion: Conversion,
}

/// A loaded, checked measured trace.
#[derive(Debug, Clone, PartialEq)]
pub struct Measured {
    pub slug: String,
    pub trace: Trace,
    pub provenance: Provenance,
    /// The BLAKE3 of `trace.toml`, lowercase hex: what a scenario names it by
    /// (EMU-64, CON-27(a)).
    pub hash: String,
}

fn is_hex_digest(s: &str) -> bool {
    s.len() == 64
        && s.bytes()
            .all(|b| b.is_ascii_digit() || (b'a'..=b'f').contains(&b))
}

fn read(path: &Path) -> Result<String, TraceError> {
    std::fs::read_to_string(path).map_err(|e| TraceError {
        reason: "layout",
        message: format!("{}: {e}", path.display()),
    })
}

/// Check one sample against EMU-62; `prev` is the previous sample's time.
fn check_sample(i: usize, s: &Sample, prev: Option<f64>) -> Result<(), TraceError> {
    let rtt = s
        .rtt_ms
        .map_or([0.0; 4], |r| [r.avg, r.min, r.max, r.stdev]);
    let all = [s.t_s, s.loss, s.dl_kbps, s.ul_kbps];
    if all.iter().chain(&rtt).any(|x| !x.is_finite()) {
        return refuse("range", format!("sample {i}: a number is not finite"));
    }
    match prev {
        None if s.t_s != 0.0 => return refuse("samples", "the first sample's t_s is not 0"),
        Some(p) if s.t_s <= p => {
            return refuse("samples", format!("sample {i}: t_s does not increase"));
        }
        _ => {}
    }
    if !(0.0..=1.0).contains(&s.loss) {
        return refuse(
            "range",
            format!("sample {i}: loss {} is outside [0, 1]", s.loss),
        );
    }
    match s.rtt_ms {
        None if s.loss < 1.0 => {
            return refuse(
                "samples",
                format!("sample {i}: rtt_ms is missing below a loss of 1"),
            );
        }
        Some(_) if s.loss >= 1.0 => {
            return refuse(
                "samples",
                format!("sample {i}: rtt_ms is present at a loss of 1"),
            );
        }
        Some(r) if !(0.0 <= r.min && r.min <= r.avg && r.avg <= r.max && r.stdev >= 0.0) => {
            return refuse(
                "range",
                format!("sample {i}: rtt_ms needs 0 ≤ min ≤ avg ≤ max and stdev ≥ 0"),
            );
        }
        _ => {}
    }
    if s.dl_kbps < 0.0 || s.ul_kbps < 0.0 {
        return refuse("range", format!("sample {i}: throughput is negative"));
    }
    Ok(())
}

/// Check the provenance against EMU-61.
fn check_provenance(p: &Provenance) -> Result<(), TraceError> {
    if !REDISTRIBUTABLE.contains(&p.licence.spdx.as_str()) {
        return refuse(
            "licence",
            format!(
                "`{}` is not a licence that allows redistribution ({REDISTRIBUTABLE:?})",
                p.licence.spdx
            ),
        );
    }
    let c = &p.collection;
    if !["testbed", "commercial"].contains(&c.network.as_str()) {
        return refuse(
            "provenance",
            "collection.network is `testbed` or `commercial`",
        );
    }
    if !["static", "walk", "drive"].contains(&c.mobility.as_str()) {
        return refuse(
            "provenance",
            "collection.mobility is `static`, `walk` or `drive`",
        );
    }
    let empty = [
        ("source.title", &p.source.title),
        ("source.url", &p.source.url),
        ("licence.attribution", &p.licence.attribution),
        ("collection.date", &c.date),
        ("collection.device", &c.device),
        ("collection.technology", &c.technology),
        ("collection.location_class", &c.location_class),
        ("collection.method", &c.method),
        ("conversion.tool", &p.conversion.tool),
        ("conversion.tool_path", &p.conversion.tool_path),
    ]
    .into_iter()
    .find(|(_, v)| v.trim().is_empty());
    if let Some((k, _)) = empty {
        return refuse("provenance", format!("{k} is empty"));
    }
    if !is_hex_digest(&p.conversion.tool_blake3) {
        return refuse(
            "provenance",
            "conversion.tool_blake3 is not a BLAKE3 hex digest",
        );
    }
    if p.conversion.sources.is_empty() {
        return refuse("provenance", "conversion.sources names no source file");
    }
    if let Some(f) = p
        .conversion
        .sources
        .iter()
        .find(|f| !is_hex_digest(&f.blake3))
    {
        return refuse(
            "provenance",
            format!("source `{}`: blake3 is not a BLAKE3 hex digest", f.name),
        );
    }
    Ok(())
}

/// Load and check the measured trace in `dir` (EMU-64).
pub fn load(dir: &Path) -> Result<Measured, TraceError> {
    let slug = dir
        .file_name()
        .and_then(|n| n.to_str())
        .unwrap_or_default()
        .to_owned();
    if slug.is_empty()
        || !slug
            .bytes()
            .all(|b| b.is_ascii_lowercase() || b.is_ascii_digit() || b == b'-')
    {
        return refuse("layout", format!("`{slug}` is not a slug (a-z, 0-9, -)"));
    }
    let entries = std::fs::read_dir(dir).map_err(|e| TraceError {
        reason: "layout",
        message: format!("{}: {e}", dir.display()),
    })?;
    let mut names: Vec<String> = Vec::new();
    for e in entries {
        let e = e.map_err(|e| TraceError {
            reason: "layout",
            message: e.to_string(),
        })?;
        names.push(e.file_name().to_string_lossy().into_owned());
    }
    names.sort();
    if names != ["provenance.toml", "trace.toml"] {
        return refuse(
            "layout",
            format!("{slug} holds {names:?}, not exactly trace.toml and provenance.toml"),
        );
    }
    let trace_text = read(&dir.join("trace.toml"))?;
    let trace: Trace = toml::from_str(&trace_text).map_err(|e| TraceError {
        reason: "parse",
        message: format!("trace.toml: {e}"),
    })?;
    let provenance: Provenance =
        toml::from_str(&read(&dir.join("provenance.toml"))?).map_err(|e| TraceError {
            reason: "parse",
            message: format!("provenance.toml: {e}"),
        })?;
    if trace.schema_version != 1 {
        return refuse("parse", "trace.toml: schema_version is 1");
    }
    if !(trace.sample_interval_s.is_finite() && trace.sample_interval_s > 0.0) {
        return refuse("range", "sample_interval_s is positive");
    }
    if trace.samples.len() < 2 {
        return refuse("samples", "a trace holds at least two samples");
    }
    let mut prev = None;
    for (i, s) in trace.samples.iter().enumerate() {
        check_sample(i, s, prev)?;
        prev = Some(s.t_s);
    }
    check_provenance(&provenance)?;
    Ok(Measured {
        slug,
        trace,
        provenance,
        hash: blake3::hash(trace_text.as_bytes()).to_hex().to_string(),
    })
}
