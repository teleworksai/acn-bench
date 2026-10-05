//! `cargo xtask import-5g-iana` (SPEC 020 EMU-65): convert the PING file of the
//! 5G-IANA drive test (Zenodo record 12664724, CC BY 4.0) into a measured trace
//! (EMU-62), dropping what EMU-63 forbids. The conversion is deterministic, so
//! `--check` can show that the committed `trace.toml` is this tool's output.
//!
//! The source is KML written by a drive-test app: one `<Placemark>` per test,
//! with `<Data name="…"><value>N unit</value></Data>` fields. It is read with
//! plain string search; anything off that shape is refused rather than guessed.

use std::path::{Path, PathBuf};

use acn_emu::trace::{parse_provenance, parse_trace};
use serde::Serialize;

use crate::{Error, Result};

/// This file, which `provenance.toml` names as the conversion tool (EMU-61).
pub const TOOL_PATH: &str = "crates/xtask/src/import_5g_iana.rs";
/// The source file name `provenance.toml` records the hash of.
pub const SOURCE_NAME: &str = "2023.01.29_OnePlus9-2_datatest_PING.kml";

fn invalid(msg: impl Into<String>) -> Error {
    Error::Invalid(msg.into())
}

/// The value of `<Data name="{name}">` in a placemark, if the field is present.
/// A field present in any other shape, or twice, is refused.
fn field<'a>(placemark: &'a str, name: &str) -> Result<Option<&'a str>> {
    let open = format!("<Data name=\"{name}\">");
    let Some(at) = placemark.find(&open) else {
        return Ok(None);
    };
    let after = &placemark[at + open.len()..];
    if after.contains(&open) {
        return Err(invalid(format!("field `{name}` appears twice in a test")));
    }
    let bad = || invalid(format!("field `{name}` is not `<value>…</value></Data>`"));
    let rest = after.strip_prefix("<value>").ok_or_else(bad)?;
    let len = rest.find("</value></Data>").ok_or_else(bad)?;
    Ok(Some(&rest[..len]))
}

/// A number with its unit, such as `36 ms`.
fn number(placemark: &str, name: &str, unit: &str) -> Result<Option<f64>> {
    let Some(v) = field(placemark, name)? else {
        return Ok(None);
    };
    let n = v
        .strip_suffix(unit)
        .map(str::trim_end)
        .ok_or_else(|| invalid(format!("`{name}` = `{v}` does not end in `{unit}`")))?;
    let x: f64 = n
        .parse()
        .map_err(|_| invalid(format!("`{name}` = `{v}` is not a number")))?;
    if !x.is_finite() {
        return Err(invalid(format!("`{name}` = `{v}` is not finite")));
    }
    Ok(Some(x))
}

fn required(placemark: &str, name: &str, unit: &str) -> Result<f64> {
    number(placemark, name, unit)?.ok_or_else(|| invalid(format!("a test lacks `{name}`")))
}

/// Days since 1970-01-01 of a proleptic Gregorian date (Howard Hinnant's
/// `days_from_civil`).
fn days_from_civil(y: i64, m: i64, d: i64) -> i64 {
    let y = if m <= 2 { y - 1 } else { y };
    let era = y.div_euclid(400);
    let yoe = y - era * 400;
    let mp = (m + 9) % 12;
    let doy = (153 * mp + 2) / 5 + d - 1;
    let doe = yoe * 365 + yoe / 4 - yoe / 100 + doy;
    era * 146_097 + doe - 719_468
}

fn days_in_month(y: i64, m: i64) -> i64 {
    let leap = (y % 4 == 0 && y % 100 != 0) || y % 400 == 0;
    match m {
        4 | 6 | 9 | 11 => 30,
        2 if leap => 29,
        2 => 28,
        _ => 31,
    }
}

/// `TIME`, as `2023.01.29_12.05.40`, in seconds since 1970 (no zone: only
/// differences are kept).
fn time(placemark: &str) -> Result<i64> {
    let v = field(placemark, "TIME")?.ok_or_else(|| invalid("a test lacks `TIME`"))?;
    let bad = || invalid(format!("`TIME` = `{v}` is not YYYY.MM.DD_hh.mm.ss"));
    let (date, clock) = v.split_once('_').ok_or_else(bad)?;
    let parse = |s: &str, widths: [usize; 3]| -> Result<[i64; 3]> {
        let parts: Vec<&str> = s.split('.').collect();
        if parts.len() != 3 {
            return Err(bad());
        }
        let mut out = [0; 3];
        for (i, p) in parts.iter().enumerate() {
            if p.len() != widths[i] || !p.bytes().all(|b| b.is_ascii_digit()) {
                return Err(bad());
            }
            out[i] = p.parse().map_err(|_| bad())?;
        }
        Ok(out)
    };
    let [y, mo, d] = parse(date, [4, 2, 2])?;
    let [h, mi, s] = parse(clock, [2, 2, 2])?;
    let date_ok = (1..=12).contains(&mo) && (1..=days_in_month(y, mo)).contains(&d);
    if !date_ok || h > 23 || mi > 59 || s > 59 {
        return Err(bad());
    }
    Ok(days_from_civil(y, mo, d) * 86_400 + h * 3_600 + mi * 60 + s)
}

/// One converted sample, before formatting.
struct Row {
    t: i64,
    loss: f64,
    dl: f64,
    ul: f64,
    rtt: Option<[f64; 4]>,
}

/// What a conversion produced.
#[derive(Debug, Clone, PartialEq, Serialize)]
pub struct Converted {
    /// The bytes of `trace.toml`.
    pub trace: String,
    pub samples: usize,
    /// Placemarks that ran no ping test, and so were dropped.
    pub skipped: usize,
}

/// `{:?}` of an `f64` is its shortest round-trip form, with `.0` on integers:
/// a valid TOML float that reads back to the same value.
fn num(x: f64) -> String {
    format!("{x:?}")
}

/// The placemarks of the source, each the text between its tags. Every
/// `<Placemark` must be exactly `<Placemark>` and close before the next opens.
fn placemarks(kml: &str) -> Result<Vec<&str>> {
    let mut out = Vec::new();
    let mut rest = kml;
    while let Some(start) = rest.find("<Placemark") {
        let body = rest[start..]
            .strip_prefix("<Placemark>")
            .ok_or_else(|| invalid("a <Placemark> tag has attributes or another shape"))?;
        let end = body
            .find("</Placemark>")
            .ok_or_else(|| invalid("a <Placemark> is not closed"))?;
        if body[..end].contains("<Placemark") {
            return Err(invalid("a <Placemark> is not closed before the next opens"));
        }
        out.push(&body[..end]);
        rest = &body[end + "</Placemark>".len()..];
    }
    Ok(out)
}

/// Convert the text of `PING.kml` to the bytes of `trace.toml`, checked
/// against EMU-62 before it is returned.
pub fn convert(kml: &str) -> Result<Converted> {
    let mut rows: Vec<Row> = Vec::new();
    let mut skipped = 0;
    for p in placemarks(kml)? {
        // A placemark with no ping test (the source's first) has no loss and no
        // round-trip time: it says nothing about the link that EMU-62 can hold.
        let Some(loss_pct) = number(p, "PING LOSS", "%")? else {
            if p.contains(" PING\">") {
                return Err(invalid("a test has ping times but no `PING LOSS`"));
            }
            skipped += 1;
            continue;
        };
        if !(0.0..=100.0).contains(&loss_pct) {
            return Err(invalid(format!(
                "`PING LOSS` = {loss_pct} % is outside 0..100"
            )));
        }
        let rtt = match number(p, "AVG PING", "ms")? {
            Some(avg) => Some([
                avg,
                required(p, "MIN PING", "ms")?,
                required(p, "MAX PING", "ms")?,
                required(p, "STDEV PING", "ms")?,
            ]),
            None => None,
        };
        rows.push(Row {
            t: time(p)?,
            loss: loss_pct / 100.0,
            dl: required(p, "TEST DL", "kbps")?,
            ul: required(p, "TEST UL", "kbps")?,
            rtt,
        });
    }
    if rows.len() < 2 {
        return Err(invalid("fewer than two placemarks ran a ping test"));
    }
    let t0 = rows[0].t;
    let mut gaps: Vec<i64> = rows.windows(2).map(|w| w[1].t - w[0].t).collect();
    if let Some(i) = gaps.iter().position(|g| *g <= 0) {
        return Err(invalid(format!(
            "test {} is not later than the one before",
            i + 1
        )));
    }
    gaps.sort_unstable();
    // The nominal spacing: the lower median of the gaps between tests.
    let interval = gaps[(gaps.len() - 1) / 2];

    let mut out = String::new();
    out.push_str(
        "# A measured trace (SPEC 020 EMU-62), written by `cargo xtask import-5g-iana`\n\
         # from PING.kml of Zenodo record 12664724. Do not edit: see provenance.toml.\n",
    );
    out.push_str("schema_version = 1\n");
    out.push_str(&format!("sample_interval_s = {}\n", num(interval as f64)));
    for r in &rows {
        out.push_str("\n[[sample]]\n");
        out.push_str(&format!("t_s = {}\n", num((r.t - t0) as f64)));
        out.push_str(&format!("loss = {}\n", num(r.loss)));
        out.push_str(&format!("dl_kbps = {}\n", num(r.dl)));
        out.push_str(&format!("ul_kbps = {}\n", num(r.ul)));
        if let Some([avg, min, max, stdev]) = r.rtt {
            out.push_str(&format!(
                "rtt_ms = {{ avg = {}, min = {}, max = {}, stdev = {} }}\n",
                num(avg),
                num(min),
                num(max),
                num(stdev)
            ));
        }
    }
    // The output must be a trace the loader accepts (EMU-62, EMU-65): a source
    // whose values break a rule is refused here, not written.
    parse_trace(&out).map_err(|e| invalid(format!("the converted trace is refused: {e}")))?;
    Ok(Converted {
        trace: out,
        samples: rows.len(),
        skipped,
    })
}

/// The result of `cargo xtask import-5g-iana`.
#[derive(Debug, Clone, PartialEq, Serialize)]
pub struct Report {
    pub ok: bool,
    pub trace: PathBuf,
    pub samples: usize,
    pub skipped: usize,
    /// The BLAKE3 of `trace.toml` (EMU-64).
    pub hash: String,
    /// `check`: the committed bytes match; `write`: they were written.
    pub mode: &'static str,
}

fn read(path: &Path) -> Result<Vec<u8>> {
    std::fs::read(path).map_err(|e| Error::io(path, e))
}

/// Write `bytes` to `path` through a temporary file in the same directory and
/// a rename, so a failure never leaves a truncated trace that still parses.
fn write_atomic(path: &Path, bytes: &[u8]) -> Result<()> {
    let tmp = path.with_extension(format!("toml.tmp-{}", std::process::id()));
    let result = std::fs::write(&tmp, bytes).and_then(|()| std::fs::rename(&tmp, path));
    if let Err(e) = result {
        let _ = std::fs::remove_file(&tmp);
        return Err(Error::io(path, e));
    }
    Ok(())
}

/// Convert `ping` into `dir/trace.toml`, or with `check`, refuse unless the
/// committed bytes are the conversion's. Either way the source must hash to the
/// value `dir/provenance.toml` records, and this tool must hash to the value it
/// records for the tool (EMU-65).
pub fn run(root: &Path, ping: &Path, dir: &Path, check: bool) -> Result<Report> {
    let prov_path = dir.join("provenance.toml");
    let text = String::from_utf8(read(&prov_path)?)
        .map_err(|_| invalid(format!("{} is not UTF-8", prov_path.display())))?;
    let c = parse_provenance(&text)
        .map_err(|e| invalid(format!("{}: {e}", prov_path.display())))?
        .conversion;
    if c.tool_path != TOOL_PATH {
        return Err(invalid(format!(
            "provenance names the tool `{}`, not `{TOOL_PATH}`",
            c.tool_path
        )));
    }
    let tool = blake3::hash(&read(&root.join(TOOL_PATH))?)
        .to_hex()
        .to_string();
    if tool != c.tool_blake3 {
        return Err(invalid(format!(
            "{TOOL_PATH} hashes to {tool}, not the {} provenance records. The tool \
             changed: set conversion.tool_blake3 to {tool}, then re-run it to rewrite \
             trace.toml (an env-change)",
            c.tool_blake3
        )));
    }
    let recorded = c
        .sources
        .iter()
        .find(|s| s.name == SOURCE_NAME)
        .ok_or_else(|| invalid(format!("provenance records no source `{SOURCE_NAME}`")))?;
    let source = read(ping)?;
    let actual = blake3::hash(&source).to_hex().to_string();
    if actual != recorded.blake3 {
        return Err(invalid(format!(
            "{} hashes to {actual}, not the {} provenance records for {SOURCE_NAME}",
            ping.display(),
            recorded.blake3
        )));
    }
    let kml = String::from_utf8(source)
        .map_err(|_| invalid(format!("{} is not UTF-8", ping.display())))?;
    let conv = convert(&kml)?;
    let trace = dir.join("trace.toml");
    if check {
        let committed = read(&trace)?;
        if committed != conv.trace.as_bytes() {
            return Err(invalid(format!(
                "{} is not this tool's output for {}",
                trace.display(),
                ping.display()
            )));
        }
    } else {
        write_atomic(&trace, conv.trace.as_bytes())?;
    }
    Ok(Report {
        ok: true,
        hash: blake3::hash(conv.trace.as_bytes()).to_hex().to_string(),
        trace,
        samples: conv.samples,
        skipped: conv.skipped,
        mode: if check { "check" } else { "write" },
    })
}

#[cfg(test)]
mod tests {
    use super::*;

    /// Cites: EMU-65
    #[test]
    fn days_from_civil_matches_known_dates() {
        assert_eq!(days_from_civil(1970, 1, 1), 0);
        assert_eq!(days_from_civil(2000, 3, 1), 11_017);
        assert_eq!(days_from_civil(2023, 1, 29), 19_386);
    }

    /// Cites: EMU-65
    #[test]
    fn a_time_off_the_format_is_refused() {
        let at = |v: &str| format!("<Data name=\"TIME\"><value>{v}</value></Data>");
        assert_eq!(
            time(&at("2023.01.29_12.05.40")).ok(),
            Some(19_386 * 86_400 + 43_540)
        );
        for bad in [
            "2023-01-29 12:05:40",
            "2023.13.29_12.05.40",
            "2023.02.29_12.05.40",
            "2023.04.31_12.05.40",
            "2023.01.29_-1.06.30",
            "2023.01.29_12.60.00",
            "2023.01.29_12.05.4",
            "2023.01.29_12.05.40.1",
        ] {
            assert!(time(&at(bad)).is_err(), "{bad}");
        }
    }

    /// Cites: EMU-65
    #[test]
    fn a_field_off_the_published_shape_is_refused() {
        assert!(field("<Data name=\"X\"> <value>1</value></Data>", "X").is_err());
        assert!(field("<Data name=\"X\"><value>1</value> </Data>", "X").is_err());
        let twice =
            "<Data name=\"X\"><value>1</value></Data><Data name=\"X\"><value>2</value></Data>";
        assert!(field(twice, "X").is_err());
        assert_eq!(
            field("<Data name=\"X\"><value>1 ms</value></Data>", "X").ok(),
            Some(Some("1 ms"))
        );
        assert_eq!(
            field("<Data name=\"Y\"><value>1</value></Data>", "X").ok(),
            Some(None)
        );
    }
}
