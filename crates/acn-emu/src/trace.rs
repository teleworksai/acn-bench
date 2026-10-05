//! Measured impairment traces (SPEC 020 §6, EMU-60 to EMU-64; CON-21): a
//! directory `scenarios/measured/<slug>/` holding `trace.toml`, a series of
//! link-state samples, and `provenance.toml`, where the series came from.
//! Loading checks both, so that a trace without its provenance, or with a field
//! that could carry an identifier, never enters a scenario.

use std::collections::BTreeSet;
use std::fmt;
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

/// The named reasons a trace is refused for (EMU-64).
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum Reason {
    /// The directory, its name or its entries break EMU-60.
    Layout,
    /// A file is not TOML of the EMU-61 or EMU-62 shape, including an unknown
    /// or missing key.
    Parse,
    /// The samples break an ordering or presence rule of EMU-62.
    Samples,
    /// A number is out of its EMU-62 range or not finite.
    Range,
    /// The licence does not allow redistribution (EMU-61).
    Licence,
    /// A provenance value breaks EMU-61.
    Provenance,
}

impl Reason {
    /// The reason's name, as the CLI reports it.
    pub fn name(self) -> &'static str {
        match self {
            Self::Layout => "layout",
            Self::Parse => "parse",
            Self::Samples => "samples",
            Self::Range => "range",
            Self::Licence => "licence",
            Self::Provenance => "provenance",
        }
    }
}

impl fmt::Display for Reason {
    fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
        f.write_str(self.name())
    }
}

/// Why a trace was refused.
#[derive(Debug, Clone, PartialEq, Eq, thiserror::Error)]
#[error("{reason}: {message}")]
pub struct TraceError {
    pub reason: Reason,
    pub message: String,
}

fn refuse<T>(reason: Reason, message: impl Into<String>) -> Result<T, TraceError> {
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
    /// `YYYY-MM-DD`.
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

/// Whether `s` is a lowercase-hex BLAKE3 digest.
pub fn is_hex_digest(s: &str) -> bool {
    s.len() == 64
        && s.bytes()
            .all(|b| b.is_ascii_digit() || (b'a'..=b'f').contains(&b))
}

/// Whether `s` is a calendar date `YYYY-MM-DD`.
fn is_date(s: &str) -> bool {
    let b = s.as_bytes();
    if b.len() != 10 || b[4] != b'-' || b[7] != b'-' {
        return false;
    }
    let num = |r: std::ops::Range<usize>| -> Option<u32> {
        let part = s.get(r)?;
        if part.bytes().all(|c| c.is_ascii_digit()) {
            part.parse().ok()
        } else {
            None
        }
    };
    let (Some(y), Some(m), Some(d)) = (num(0..4), num(5..7), num(8..10)) else {
        return false;
    };
    let leap = (y % 4 == 0 && y % 100 != 0) || y % 400 == 0;
    let days = match m {
        1 | 3 | 5 | 7 | 8 | 10 | 12 => 31,
        4 | 6 | 9 | 11 => 30,
        2 if leap => 29,
        2 => 28,
        _ => return false,
    };
    (1..=days).contains(&d)
}

fn read(path: &Path) -> Result<String, TraceError> {
    std::fs::read_to_string(path).map_err(|e| TraceError {
        reason: Reason::Layout,
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
        return refuse(Reason::Range, format!("sample {i}: a number is not finite"));
    }
    match prev {
        // `-0.0 == 0.0`, so compare bits: the first time is exactly `0.0`.
        None if s.t_s.to_bits() != 0 => {
            return refuse(Reason::Samples, "the first sample's t_s is not 0");
        }
        Some(p) if s.t_s <= p => {
            return refuse(
                Reason::Samples,
                format!("sample {i}: t_s does not strictly increase"),
            );
        }
        _ => {}
    }
    if !(0.0..=1.0).contains(&s.loss) {
        return refuse(
            Reason::Range,
            format!("sample {i}: loss {} is outside [0, 1]", s.loss),
        );
    }
    match s.rtt_ms {
        None if s.loss < 1.0 => {
            return refuse(
                Reason::Samples,
                format!("sample {i}: rtt_ms is missing below a loss of 1"),
            );
        }
        Some(_) if s.loss >= 1.0 => {
            return refuse(
                Reason::Samples,
                format!("sample {i}: rtt_ms is present at a loss of 1"),
            );
        }
        Some(r) if !(0.0 <= r.min && r.min <= r.avg && r.avg <= r.max && r.stdev >= 0.0) => {
            return refuse(
                Reason::Range,
                format!("sample {i}: rtt_ms needs 0 ≤ min ≤ avg ≤ max and stdev ≥ 0"),
            );
        }
        _ => {}
    }
    if s.dl_kbps < 0.0 || s.ul_kbps < 0.0 {
        return refuse(Reason::Range, format!("sample {i}: throughput is negative"));
    }
    Ok(())
}

/// Parse and check the text of a `trace.toml` against EMU-62. The importer of
/// EMU-65 runs this on its output before writing it.
pub fn parse_trace(text: &str) -> Result<Trace, TraceError> {
    let trace: Trace = toml::from_str(text).map_err(|e| TraceError {
        reason: Reason::Parse,
        message: format!("trace.toml: {e}"),
    })?;
    if trace.schema_version != 1 {
        return refuse(Reason::Parse, "trace.toml: schema_version is not 1");
    }
    if !(trace.sample_interval_s.is_finite() && trace.sample_interval_s > 0.0) {
        return refuse(Reason::Range, "sample_interval_s is not a positive number");
    }
    if trace.samples.len() < 2 {
        return refuse(Reason::Samples, "a trace holds at least two samples");
    }
    let mut prev = None;
    for (i, s) in trace.samples.iter().enumerate() {
        check_sample(i, s, prev)?;
        prev = Some(s.t_s);
    }
    Ok(trace)
}

/// Parse `provenance.toml` into its EMU-61 shape, without the value checks.
pub fn parse_provenance(text: &str) -> Result<Provenance, TraceError> {
    toml::from_str(text).map_err(|e| TraceError {
        reason: Reason::Parse,
        message: format!("provenance.toml: {e}"),
    })
}

/// Check the provenance against EMU-61.
fn check_provenance(p: &Provenance) -> Result<(), TraceError> {
    if !REDISTRIBUTABLE.contains(&p.licence.spdx.as_str()) {
        return refuse(
            Reason::Licence,
            format!(
                "`{}` is not a licence that allows redistribution ({REDISTRIBUTABLE:?})",
                p.licence.spdx
            ),
        );
    }
    let c = &p.collection;
    if !["testbed", "commercial"].contains(&c.network.as_str()) {
        return refuse(
            Reason::Provenance,
            "collection.network is `testbed` or `commercial`",
        );
    }
    if !["static", "walk", "drive"].contains(&c.mobility.as_str()) {
        return refuse(
            Reason::Provenance,
            "collection.mobility is `static`, `walk` or `drive`",
        );
    }
    if !is_date(&c.date) {
        return refuse(
            Reason::Provenance,
            format!("collection.date `{}` is not a date YYYY-MM-DD", c.date),
        );
    }
    let identifier = p.source.identifier.as_deref();
    let texts = [
        ("source.title", Some(p.source.title.as_str())),
        ("source.url", Some(p.source.url.as_str())),
        ("source.identifier", identifier),
        ("licence.attribution", Some(p.licence.attribution.as_str())),
        ("collection.device", Some(c.device.as_str())),
        ("collection.technology", Some(c.technology.as_str())),
        ("collection.location_class", Some(c.location_class.as_str())),
        ("collection.method", Some(c.method.as_str())),
        ("conversion.tool", Some(p.conversion.tool.as_str())),
        (
            "conversion.tool_path",
            Some(p.conversion.tool_path.as_str()),
        ),
    ];
    if let Some((k, _)) = texts
        .iter()
        .find(|(_, v)| v.is_some_and(|v| v.trim().is_empty()))
    {
        return refuse(Reason::Provenance, format!("{k} is empty"));
    }
    if !is_hex_digest(&p.conversion.tool_blake3) {
        return refuse(
            Reason::Provenance,
            "conversion.tool_blake3 is not a BLAKE3 hex digest",
        );
    }
    if p.conversion.sources.is_empty() {
        return refuse(
            Reason::Provenance,
            "conversion.sources names no source file",
        );
    }
    let mut names = BTreeSet::new();
    for f in &p.conversion.sources {
        if f.name.trim().is_empty() {
            return refuse(Reason::Provenance, "a conversion source has an empty name");
        }
        if !names.insert(f.name.as_str()) {
            return refuse(
                Reason::Provenance,
                format!("source `{}` is listed twice", f.name),
            );
        }
        if !is_hex_digest(&f.blake3) {
            return refuse(
                Reason::Provenance,
                format!("source `{}`: blake3 is not a BLAKE3 hex digest", f.name),
            );
        }
    }
    if p.conversion.dropped.is_empty() || p.conversion.dropped.iter().any(|d| d.trim().is_empty()) {
        return refuse(
            Reason::Provenance,
            "conversion.dropped must state each thing dropped, and why",
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
        return refuse(
            Reason::Layout,
            format!("`{slug}` is not a slug (a-z, 0-9, -)"),
        );
    }
    let layout = |e: std::io::Error| TraceError {
        reason: Reason::Layout,
        message: format!("{}: {e}", dir.display()),
    };
    let mut names: Vec<String> = Vec::new();
    for e in std::fs::read_dir(dir).map_err(layout)? {
        let e = e.map_err(layout)?;
        let name = e.file_name().to_string_lossy().into_owned();
        // `file_type` does not follow symlinks: the bytes hashed must be the
        // bytes the repository holds.
        if !e.file_type().map_err(layout)?.is_file() {
            return refuse(
                Reason::Layout,
                format!("{slug}/{name} is not a regular file"),
            );
        }
        names.push(name);
    }
    names.sort();
    if names != ["provenance.toml", "trace.toml"] {
        return refuse(
            Reason::Layout,
            format!("{slug} holds {names:?}, not exactly trace.toml and provenance.toml"),
        );
    }
    let trace_text = read(&dir.join("trace.toml"))?;
    let provenance = parse_provenance(&read(&dir.join("provenance.toml"))?)?;
    let trace = parse_trace(&trace_text)?;
    check_provenance(&provenance)?;
    Ok(Measured {
        slug,
        trace,
        provenance,
        hash: blake3::hash(trace_text.as_bytes()).to_hex().to_string(),
    })
}

#[cfg(test)]
mod tests {
    use super::*;

    /// Cites: EMU-61
    #[test]
    fn dates_are_calendar_dates() {
        for ok in ["2023-01-29", "2024-02-29", "2000-02-29"] {
            assert!(is_date(ok), "{ok}");
        }
        for bad in [
            "2023-02-29",
            "1900-02-29",
            "2023-04-31",
            "2023-13-01",
            "2023-00-10",
            "2023-1-29",
            "yesterday",
            "+023-01-29",
        ] {
            assert!(!is_date(bad), "{bad}");
        }
    }
}
