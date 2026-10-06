//! Synthetic scenario files (SPEC 020 §3, EMU-20 to EMU-22): one TOML file
//! under `scenarios/synthetic/` naming a run's links and their stages. Loading
//! refuses anything off the fixed shape by a named reason and returns the
//! file's hash (CON-27(a)) with the link parameters, from which links are built
//! under a replicate seed.

use std::collections::BTreeSet;
use std::path::Path;

use serde::Deserialize;

use crate::link::{
    Delay, Direction, Link, LinkError, LinkSpec, Loss, OutageCause, OutageMode, Rate, Reorder,
    TraceSchedule, Window, is_name,
};

/// Why a scenario was refused (EMU-21).
#[derive(Debug, Clone, PartialEq, Eq, thiserror::Error)]
#[error("{reason}: {message}")]
pub struct ScenarioError {
    /// From `load` (EMU-21): `layout` (the file cannot be read or is not
    /// `.toml`), `parse` (not TOML of the EMU-20 shape, including a missing
    /// `window` list), `name` (a name, or a `name` other than the file stem),
    /// `duplicate` (two links with one name and direction), `range` (a
    /// parameter out of range, including an empty `window` list) or `trace`
    /// (EMU-22). From `Scenario::build`: `stream` (a sub-stream).
    pub reason: &'static str,
    pub message: String,
}

fn refuse<T>(reason: &'static str, message: impl Into<String>) -> Result<T, ScenarioError> {
    Err(ScenarioError {
        reason,
        message: message.into(),
    })
}

impl From<LinkError> for ScenarioError {
    fn from(e: LinkError) -> Self {
        Self {
            reason: e.reason,
            message: e.message,
        }
    }
}

#[derive(Debug, Deserialize)]
#[serde(deny_unknown_fields)]
struct File {
    schema_version: u32,
    name: String,
    #[serde(rename = "link")]
    links: Vec<LinkToml>,
}

#[derive(Debug, Deserialize)]
#[serde(deny_unknown_fields)]
struct LinkToml {
    name: String,
    direction: DirectionToml,
    outage: Option<OutageToml>,
    loss: Option<LossToml>,
    rate: Option<RateToml>,
    delay: Option<DelayToml>,
    reorder: Option<ReorderToml>,
    trace: Option<TraceToml>,
}

#[derive(Debug, Clone, Copy, Deserialize)]
#[serde(rename_all = "lowercase")]
enum DirectionToml {
    Up,
    Down,
}

#[derive(Debug, Deserialize)]
#[serde(deny_unknown_fields)]
struct OutageToml {
    window: Vec<WindowToml>,
}

#[derive(Debug, Deserialize)]
#[serde(deny_unknown_fields)]
struct WindowToml {
    start_ms: u64,
    end_ms: u64,
    mode: ModeToml,
    cause: CauseToml,
}

#[derive(Debug, Clone, Copy, Deserialize)]
#[serde(rename_all = "lowercase")]
enum ModeToml {
    Drop,
    Hold,
}

#[derive(Debug, Clone, Copy, Deserialize)]
#[serde(rename_all = "lowercase")]
enum CauseToml {
    Handover,
    Scheduled,
}

/// One table for both loss kinds: the keys of the other kind are refused by
/// name rather than by serde's generic message.
#[derive(Debug, Deserialize)]
#[serde(deny_unknown_fields)]
struct LossToml {
    kind: String,
    loss_ppm: Option<u64>,
    p_good_bad_ppm: Option<u64>,
    p_bad_good_ppm: Option<u64>,
    loss_good_ppm: Option<u64>,
    loss_bad_ppm: Option<u64>,
}

#[derive(Debug, Deserialize)]
#[serde(deny_unknown_fields)]
struct RateToml {
    rate_kbps: u64,
    burst_bytes: u64,
    queue_bytes: u64,
}

#[derive(Debug, Deserialize)]
#[serde(deny_unknown_fields)]
struct DelayToml {
    delay_us: u64,
    jitter_us: u64,
}

#[derive(Debug, Deserialize)]
#[serde(deny_unknown_fields)]
struct ReorderToml {
    reorder_ppm: u64,
    gap_us: u64,
}

#[derive(Debug, Deserialize)]
#[serde(deny_unknown_fields)]
struct TraceToml {
    dir: String,
    blake3: String,
    #[serde(default)]
    start_s: u64,
    burst_bytes: u64,
    queue_bytes: u64,
}

/// A loaded, checked scenario (EMU-21).
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct Scenario {
    pub name: String,
    /// The BLAKE3 of the file's bytes, lowercase hex (CON-27(a)).
    pub hash: String,
    /// The links, in file order.
    pub links: Vec<LinkSpec>,
}

impl Scenario {
    /// Build every link under `replicate_seed`, in file order (EMU-21).
    pub fn build(&self, replicate_seed: u64) -> Result<Vec<Link>, ScenarioError> {
        self.links
            .iter()
            .map(|s| Link::new(s.clone(), replicate_seed).map_err(ScenarioError::from))
            .collect()
    }
}

/// Nanoseconds from a count of `unit_ns`, refused on overflow.
fn ns(v: u64, unit_ns: u64, what: &str) -> Result<i64, ScenarioError> {
    v.checked_mul(unit_ns)
        .and_then(|x| i64::try_from(x).ok())
        .map_or_else(|| refuse("range", format!("{what} {v} overflows")), Ok)
}

fn loss(l: &LossToml, link: &str) -> Result<Loss, ScenarioError> {
    let ge = [
        ("p_good_bad_ppm", l.p_good_bad_ppm),
        ("p_bad_good_ppm", l.p_bad_good_ppm),
        ("loss_good_ppm", l.loss_good_ppm),
        ("loss_bad_ppm", l.loss_bad_ppm),
    ];
    match l.kind.as_str() {
        "iid" => {
            if let Some((k, _)) = ge.iter().find(|(_, v)| v.is_some()) {
                return refuse(
                    "parse",
                    format!("link {link}: `{k}` is not a key of iid loss"),
                );
            }
            let loss_ppm = l.loss_ppm.ok_or_else(|| ScenarioError {
                reason: "parse",
                message: format!("link {link}: iid loss needs `loss_ppm`"),
            })?;
            Ok(Loss::Iid { loss_ppm })
        }
        "gilbert_elliott" => {
            if l.loss_ppm.is_some() {
                return refuse(
                    "parse",
                    format!("link {link}: `loss_ppm` is not a key of gilbert_elliott loss"),
                );
            }
            let get = |k: &str, v: Option<u64>| {
                v.ok_or_else(|| ScenarioError {
                    reason: "parse",
                    message: format!("link {link}: gilbert_elliott loss needs `{k}`"),
                })
            };
            Ok(Loss::GilbertElliott {
                p_good_bad_ppm: get(ge[0].0, ge[0].1)?,
                p_bad_good_ppm: get(ge[1].0, ge[1].1)?,
                loss_good_ppm: get(ge[2].0, ge[2].1)?,
                loss_bad_ppm: get(ge[3].0, ge[3].1)?,
            })
        }
        other => refuse(
            "parse",
            format!("link {link}: loss kind `{other}` is not `iid` or `gilbert_elliott`"),
        ),
    }
}

/// The schedule a `[link.trace]` names (EMU-12, EMU-22): the trace loads
/// under EMU-64 and has the hash the scenario records.
fn traced(
    tr: &TraceToml,
    base: &Path,
    link: &str,
    direction: Direction,
) -> Result<TraceSchedule, ScenarioError> {
    let fail = |m: String| ScenarioError {
        reason: "trace",
        message: format!("link {link}: {m}"),
    };
    let m =
        crate::trace::load(&base.join(&tr.dir)).map_err(|e| fail(format!("{}: {e}", tr.dir)))?;
    if m.hash != tr.blake3 {
        return Err(fail(format!(
            "{} has hash {}, not the {} the scenario records",
            tr.dir, m.hash, tr.blake3
        )));
    }
    TraceSchedule::from_trace(
        &m.trace,
        direction,
        tr.start_s,
        tr.burst_bytes,
        tr.queue_bytes,
    )
    .map_err(|e| ScenarioError {
        reason: e.reason,
        message: format!("link {link}: {}", e.message),
    })
}

fn link(t: &LinkToml, base: &Path) -> Result<LinkSpec, ScenarioError> {
    if !is_name(&t.name) {
        return refuse(
            "name",
            format!("link `{}` is not a name (a-z, 0-9, -)", t.name),
        );
    }
    let mut s = LinkSpec::new(
        &t.name,
        match t.direction {
            DirectionToml::Up => Direction::Up,
            DirectionToml::Down => Direction::Down,
        },
    );
    if let Some(tr) = &t.trace {
        if t.outage.is_some() || t.loss.is_some() || t.rate.is_some() || t.delay.is_some() {
            return refuse(
                "parse",
                format!(
                    "link {}: a trace link has no outage, loss, rate or delay stage",
                    t.name
                ),
            );
        }
        s.trace = Some(traced(tr, base, &t.name, s.direction)?);
    }
    if let Some(o) = &t.outage {
        let mut ws = Vec::with_capacity(o.window.len());
        for w in &o.window {
            ws.push(Window {
                start_ns: ns(w.start_ms, 1_000_000, "start_ms")?,
                end_ns: ns(w.end_ms, 1_000_000, "end_ms")?,
                mode: match w.mode {
                    ModeToml::Drop => OutageMode::Drop,
                    ModeToml::Hold => OutageMode::Hold,
                },
                cause: match w.cause {
                    CauseToml::Handover => OutageCause::Handover,
                    CauseToml::Scheduled => OutageCause::Scheduled,
                },
            });
        }
        s.outage = Some(ws);
    }
    if let Some(l) = &t.loss {
        s.loss = Some(loss(l, &t.name)?);
    }
    if let Some(r) = &t.rate {
        s.rate = Some(Rate {
            rate_bps: r
                .rate_kbps
                .checked_mul(1_000)
                .ok_or_else(|| ScenarioError {
                    reason: "range",
                    message: format!("link {}: rate_kbps overflows", t.name),
                })?,
            burst_bytes: r.burst_bytes,
            queue_bytes: r.queue_bytes,
        });
    }
    if let Some(d) = &t.delay {
        s.delay = Some(Delay {
            delay_ns: ns(d.delay_us, 1_000, "delay_us")?,
            jitter_ns: ns(d.jitter_us, 1_000, "jitter_us")?,
        });
    }
    if let Some(r) = &t.reorder {
        s.reorder = Some(Reorder {
            reorder_ppm: r.reorder_ppm,
            gap_ns: ns(r.gap_us, 1_000, "gap_us")?,
        });
    }
    s.validate().map_err(|e| ScenarioError {
        reason: e.reason,
        message: format!("link {}: {}", t.name, e.message),
    })?;
    Ok(s)
}

/// Load and check the scenario file at `path` (EMU-20, EMU-21).
pub fn load(path: &Path) -> Result<Scenario, ScenarioError> {
    if path.extension().and_then(|e| e.to_str()) != Some("toml") {
        return refuse("layout", format!("{} is not a .toml file", path.display()));
    }
    let bytes = std::fs::read(path).map_err(|e| ScenarioError {
        reason: "layout",
        message: format!("{}: {e}", path.display()),
    })?;
    let text = std::str::from_utf8(&bytes).map_err(|_| ScenarioError {
        reason: "parse",
        message: format!("{} is not UTF-8", path.display()),
    })?;
    let f: File = toml::from_str(text).map_err(|e| ScenarioError {
        reason: "parse",
        message: format!("{}: {e}", path.display()),
    })?;
    if f.schema_version != 1 {
        return refuse("parse", "schema_version is not 1");
    }
    let stem = path
        .file_stem()
        .and_then(|s| s.to_str())
        .unwrap_or_default();
    if !is_name(&f.name) || f.name != stem {
        return refuse(
            "name",
            format!(
                "name `{}` is not a name equal to the file stem `{stem}`",
                f.name
            ),
        );
    }
    if f.links.is_empty() {
        return refuse("parse", "a scenario holds at least one [[link]]");
    }
    let mut seen = BTreeSet::new();
    let mut links = Vec::with_capacity(f.links.len());
    for t in &f.links {
        let s = link(t, path.parent().unwrap_or(Path::new(".")))?;
        if !seen.insert((s.name.clone(), s.direction)) {
            return refuse(
                "duplicate",
                format!("two links are named {} {}", s.name, s.direction),
            );
        }
        links.push(s);
    }
    Ok(Scenario {
        name: f.name,
        hash: blake3::hash(&bytes).to_hex().to_string(),
        links,
    })
}
