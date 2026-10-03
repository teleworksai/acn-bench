//! Hypothesis files (HYP-1..9): strict TOML, unknown keys rejected at every level,
//! every rule of SPEC 080 §2 checked when the file is loaded, and the status
//! decided by location and by `env-hash.json` (HYP-3).

use std::collections::{BTreeMap, BTreeSet};
use std::path::{Path, PathBuf};

use acn_trace::identity::Digest;
use serde::Deserialize;

use crate::predicate::{self, Expr, is_reserved};
use crate::{HypError, Status};

// ---- the raw file ----------------------------------------------------------

#[derive(Debug, Deserialize)]
#[serde(deny_unknown_fields)]
struct RawFile {
    poc: RawPoc,
    hypothesis: RawHypothesis,
    varies: BTreeMap<String, RawParam>,
    measures: RawMeasures,
    control: RawControl,
    design: RawDesign,
    falsifier: RawFalsifier,
    expected: RawExpected,
}

#[derive(Debug, Deserialize)]
#[serde(deny_unknown_fields)]
struct RawPoc {
    id: String,
    title: String,
    spec: Option<String>,
    report_refs: Option<Vec<String>>,
    status: Option<String>,
    supersedes: Option<String>,
}

#[derive(Debug, Deserialize)]
#[serde(deny_unknown_fields)]
struct RawHypothesis {
    statement: String,
}

#[derive(Debug, Deserialize)]
#[serde(deny_unknown_fields)]
struct RawParam {
    kind: String,
    values: Option<Vec<String>>,
    min: Option<toml::Value>,
    max: Option<toml::Value>,
    levels: Option<Vec<toml::Value>>,
    pooled: Option<bool>,
}

#[derive(Debug, Deserialize)]
#[serde(deny_unknown_fields)]
struct RawMeasures {
    primary: Vec<String>,
    secondary: Option<Vec<String>>,
}

#[derive(Debug, Deserialize)]
#[serde(deny_unknown_fields)]
struct RawControl {
    description: String,
    config: Option<toml::Table>,
    workload: Option<String>,
    inherits: Option<Vec<String>>,
}

#[derive(Debug, Deserialize)]
#[serde(deny_unknown_fields)]
struct RawDesign {
    search: String,
    replicates: i64,
    twin_required: bool,
    seeds: Option<String>,
    seed: Option<i64>,
    backends: Option<Vec<String>>,
    min_providers_for_verdict: Option<i64>,
    sim_live_tolerance: Option<BTreeMap<String, toml::Value>>,
    pins: Option<RawPins>,
}

#[derive(Debug, Deserialize)]
#[serde(deny_unknown_fields)]
struct RawPins {
    scenario: Vec<String>,
    workload: Vec<String>,
    models: BTreeMap<String, String>,
}

#[derive(Debug, Deserialize)]
#[serde(deny_unknown_fields)]
struct RawFalsifier {
    predicate: String,
    inconclusive_if: Option<String>,
}

#[derive(Debug, Deserialize)]
#[serde(deny_unknown_fields)]
struct RawExpected {
    outcome: String,
    note: Option<String>,
}

// ---- the checked file ------------------------------------------------------

/// The domain of a `[varies]` parameter (HYP-6).
#[derive(Debug, Clone, PartialEq)]
pub enum Domain {
    Bool,
    /// Values in declaration order (HYP-14 orders cells by it).
    Enum(Vec<String>),
    Range {
        min: f64,
        max: f64,
        levels: Option<Vec<f64>>,
    },
    IntRange {
        min: i64,
        max: i64,
        levels: Option<Vec<i64>>,
    },
}

impl Domain {
    /// The number of values a grid gives this parameter, if finite.
    #[must_use]
    pub fn size(&self) -> Option<usize> {
        match self {
            Self::Bool => Some(2),
            Self::Enum(v) => Some(v.len()),
            Self::Range { levels, .. } => levels.as_ref().map(Vec::len),
            Self::IntRange { levels, .. } => levels.as_ref().map(Vec::len),
        }
    }

    /// Whether `v` lies in the domain.
    #[must_use]
    pub fn admits(&self, v: &predicate::Lit) -> bool {
        use predicate::Lit;
        match (self, v) {
            (Self::Bool, Lit::Bool(_)) => true,
            (Self::Enum(vals), Lit::Ident(s)) => vals.contains(s),
            (Self::Range { min, max, .. }, Lit::Num(n)) => *min <= *n && *n <= *max,
            (Self::IntRange { min, max, .. }, Lit::Num(n)) => {
                n.fract() == 0.0 && (*min as f64) <= *n && *n <= (*max as f64)
            }
            _ => false,
        }
    }

    /// Whether the domain is numeric (it can be bounded by an `at` clause).
    #[must_use]
    pub fn is_numeric(&self) -> bool {
        matches!(self, Self::Range { .. } | Self::IntRange { .. })
    }
}

/// One `[varies]` parameter.
#[derive(Debug, Clone, PartialEq)]
pub struct Param {
    pub name: String,
    pub domain: Domain,
    /// Results are pooled across its values (HYP-24); `provider` never is.
    pub pooled: bool,
}

/// A twin tolerance (HYP-9, CON-25).
#[derive(Debug, Clone, Copy, PartialEq)]
pub enum Tolerance {
    Relative(f64),
    Absolute(f64),
}

/// `[design].pins`.
#[derive(Debug, Clone, PartialEq)]
pub struct Pins {
    pub scenario: Vec<String>,
    pub workload: Vec<String>,
    pub models: BTreeMap<String, String>,
}

/// `[design]`.
#[derive(Debug, Clone, PartialEq)]
pub struct Design {
    pub search: String,
    pub replicates: u32,
    pub twin_required: bool,
    pub seed: Option<u64>,
    pub backends: Vec<String>,
    pub min_providers_for_verdict: Option<u32>,
    pub sim_live_tolerance: BTreeMap<String, Tolerance>,
    pub pins: Option<Pins>,
}

/// `[control]`.
#[derive(Debug, Clone, PartialEq)]
pub enum Control {
    /// Values for pooled parameters; the others are inherited from the cell.
    Config(BTreeMap<String, predicate::Lit>),
    /// A generator mode, run once per assignment of `inherits`.
    Workload {
        mode: String,
        inherits: Option<Vec<String>>,
    },
    /// A candidate with neither: its verdicts are inconclusive (HYP-8).
    Missing,
}

/// A loaded, checked hypothesis file.
#[derive(Debug, Clone)]
pub struct Hypothesis {
    pub path: PathBuf,
    /// BLAKE3 of the file's bytes (HYP-5).
    pub hash: Digest,
    pub status: Status,
    pub id: String,
    pub title: String,
    pub spec: Option<String>,
    pub report_refs: Vec<String>,
    pub supersedes: Option<String>,
    pub statement: String,
    /// By name.
    pub params: BTreeMap<String, Param>,
    pub primary: Vec<String>,
    pub secondary: Vec<String>,
    pub control_description: String,
    pub control: Control,
    pub design: Design,
    pub predicate: Expr,
    pub guard: Option<Expr>,
    pub expected_outcome: String,
    pub expected_note: Option<String>,
    /// What lint and load report without failing (HYP-7, HYP-8).
    pub warnings: Vec<String>,
}

impl Hypothesis {
    /// Every quantity `[measures]` lists.
    pub fn measures(&self) -> impl Iterator<Item = &String> {
        self.primary.iter().chain(&self.secondary)
    }

    /// Whether the file has a `provider` parameter.
    #[must_use]
    pub fn has_provider(&self) -> bool {
        self.params.contains_key("provider")
    }

    /// The parameter that declares enum value `v` (values are unique, HYP-6).
    #[must_use]
    pub fn param_of_value(&self, v: &str) -> Option<&Param> {
        self.params
            .values()
            .find(|p| matches!(&p.domain, Domain::Enum(vals) if vals.iter().any(|x| x == v)))
    }
}

// ---- loading ---------------------------------------------------------------

/// Where a file was found, which decides its status (HYP-3).
#[derive(Debug, Clone)]
pub struct Location {
    pub path: PathBuf,
    pub status: Status,
    /// The workspace root (CON-28), when there is one.
    pub root: Option<PathBuf>,
}

fn io(path: &Path) -> impl Fn(std::io::Error) -> HypError + '_ {
    move |e| HypError::new(path, None, e.to_string())
}

/// HYP-3: frozen when the file lies under `<root>/hypotheses/` and `env-hash.json`
/// lists that path with this hash; candidate otherwise.
pub fn locate(path: &Path, hash: &Digest) -> Result<Location, HypError> {
    let abs = std::fs::canonicalize(path).map_err(io(path))?;
    let dir = abs.parent().unwrap_or(&abs);
    let root =
        acn_trace::env::find_root(dir).map_err(|e| HypError::new(path, None, e.to_string()))?;
    let mut status = Status::Candidate;
    if let Some(root) = &root
        && abs.starts_with(root.join("hypotheses"))
        && let Some(record) = acn_trace::env::read_record(root)
            .map_err(|e| HypError::new(path, None, e.to_string()))?
    {
        let rel = acn_trace::env::rel_path(root, &abs)
            .map_err(|e| HypError::new(path, None, e.to_string()))?;
        if record
            .files
            .iter()
            .any(|f| f.path == rel && f.blake3 == hash.to_hex())
        {
            status = Status::Frozen;
        }
    }
    Ok(Location {
        path: abs,
        status,
        root,
    })
}

/// Load a hypothesis file, read-only (HYP-4), and check it (HYP-1..14).
pub fn load(path: &Path) -> Result<Hypothesis, HypError> {
    let bytes = std::fs::read(path).map_err(io(path))?;
    let loc = locate(path, &Digest::of(&bytes))?;
    parse(&bytes, &loc)
}

fn fail<T>(path: &Path, key: &str, m: String) -> Result<T, HypError> {
    Err(HypError::new(path, Some(key.to_owned()), m))
}

fn number(v: &toml::Value) -> Option<f64> {
    match v {
        toml::Value::Float(f) => Some(*f),
        toml::Value::Integer(i) => Some(*i as f64),
        _ => None,
    }
}

fn is_hex_digest(s: &str) -> bool {
    s.len() == 64
        && s.bytes()
            .all(|b| b.is_ascii_digit() || (b'a'..=b'f').contains(&b))
}

/// Parse and check a file's bytes found at `loc` (HYP-1..14).
pub fn parse(bytes: &[u8], loc: &Location) -> Result<Hypothesis, HypError> {
    let path = loc.path.as_path();
    let frozen = loc.status == Status::Frozen;
    let text = std::str::from_utf8(bytes).map_err(|e| HypError::new(path, None, e.to_string()))?;
    // HYP-1: strict, with the key path of whatever fails.
    let raw: RawFile =
        serde_path_to_error::deserialize(toml::Deserializer::new(text)).map_err(|e| {
            let key = e.path().to_string();
            HypError::new(
                path,
                (key != ".").then_some(key),
                e.into_inner().message().to_owned(),
            )
        })?;
    let mut warnings = Vec::new();

    // HYP-2: [poc].
    let poc = &raw.poc;
    if poc.id.is_empty() || !predicate::is_ident(&poc.id) {
        return fail(path, "poc.id", format!("`{}` is not an identifier", poc.id));
    }
    let stem = path
        .file_stem()
        .and_then(|s| s.to_str())
        .unwrap_or_default();
    if stem != poc.id && !stem.starts_with(&format!("{}-", poc.id)) {
        return fail(
            path,
            "poc.id",
            format!(
                "the file stem `{stem}` must be `{0}` or `{0}-<slug>`",
                poc.id
            ),
        );
    }
    if let Some(s) = &poc.status
        && s != loc.status.as_str()
    {
        return fail(
            path,
            "poc.status",
            format!(
                "says `{s}`, but the file is {} by location and record (HYP-3)",
                loc.status.as_str()
            ),
        );
    }
    if let Some(sup) = &poc.supersedes {
        let ok = sup
            .split_once('@')
            .is_some_and(|(id, h)| predicate::is_ident(id) && is_hex_digest(h));
        if !ok {
            return fail(
                path,
                "poc.supersedes",
                format!("`{sup}` is not `<id>@<hypothesis_hash>`"),
            );
        }
    }
    if frozen {
        let Some(spec) = &poc.spec else {
            return fail(path, "poc.spec", "a frozen file names its POC spec".into());
        };
        if let Some(root) = &loc.root
            && !root.join(spec).is_file()
        {
            let file = Path::new(spec)
                .file_name()
                .and_then(|f| f.to_str())
                .unwrap_or_default();
            let listed = std::fs::read_to_string(root.join("specs/README.md"))
                .is_ok_and(|readme| !file.is_empty() && readme.contains(file));
            if !listed {
                return fail(
                    path,
                    "poc.spec",
                    format!("`{spec}` neither exists nor is listed in specs/README.md"),
                );
            }
        }
        if let Some(root) = &loc.root {
            unique_id(root, path, &poc.id)?;
        }
    }

    // HYP-6: [varies].
    let mut params = BTreeMap::new();
    let mut values_seen: BTreeMap<String, String> = BTreeMap::new();
    let grid = raw.design.search == "grid";
    for (name, p) in &raw.varies {
        let key = format!("varies.{name}");
        if is_reserved(name) {
            return fail(path, &key, format!("`{name}` is a reserved word (HYP-10)"));
        }
        let none_of = |fields: &[(&str, bool)]| -> Result<(), HypError> {
            for (f, present) in fields {
                if *present {
                    return fail(
                        path,
                        &key,
                        format!("a `{}` parameter takes no `{f}`", p.kind),
                    );
                }
            }
            Ok(())
        };
        let levels_given = p.levels.is_some();
        let domain = match p.kind.as_str() {
            "bool" => {
                none_of(&[
                    ("values", p.values.is_some()),
                    ("min", p.min.is_some()),
                    ("max", p.max.is_some()),
                    ("levels", levels_given),
                ])?;
                Domain::Bool
            }
            "enum" => {
                none_of(&[
                    ("min", p.min.is_some()),
                    ("max", p.max.is_some()),
                    ("levels", levels_given),
                ])?;
                let Some(values) = p.values.clone().filter(|v| !v.is_empty()) else {
                    return fail(
                        path,
                        &key,
                        "an enum lists its `values`, at least one".into(),
                    );
                };
                for v in &values {
                    if is_reserved(v) {
                        return fail(path, &key, format!("the value `{v}` is a reserved word"));
                    }
                    if let Some(other) = values_seen.insert(v.clone(), name.clone()) {
                        return fail(
                            path,
                            &key,
                            format!(
                                "the value `{v}` is also a value of `{other}`; enum values are unique across parameters"
                            ),
                        );
                    }
                }
                Domain::Enum(values)
            }
            "range" | "int_range" => {
                none_of(&[("values", p.values.is_some())])?;
                let int = p.kind == "int_range";
                let read = |v: &Option<toml::Value>, f: &str| -> Result<f64, HypError> {
                    let Some(v) = v else {
                        return fail(path, &key, format!("a `{}` states `{f}`", p.kind));
                    };
                    match (int, v) {
                        (true, toml::Value::Integer(i)) => Ok(*i as f64),
                        (false, v) => number(v).filter(|x| x.is_finite()).map_or_else(
                            || fail(path, &key, format!("`{f}` is not a finite number")),
                            Ok,
                        ),
                        _ => fail(path, &key, format!("`{f}` of an int_range is an integer")),
                    }
                };
                let (min, max) = (read(&p.min, "min")?, read(&p.max, "max")?);
                if min > max {
                    return fail(path, &key, format!("min {min} exceeds max {max}"));
                }
                let levels = match &p.levels {
                    None => None,
                    Some(ls) => {
                        if ls.is_empty() {
                            return fail(path, &key, "`levels` is empty".into());
                        }
                        let mut out = Vec::new();
                        for l in ls {
                            let v = match (int, l) {
                                (true, toml::Value::Integer(i)) => *i as f64,
                                (true, _) => {
                                    return fail(
                                        path,
                                        &key,
                                        "an int_range's levels are integers".into(),
                                    );
                                }
                                (false, l) => number(l).filter(|x| x.is_finite()).map_or_else(
                                    || fail(path, &key, "a level is not a finite number".into()),
                                    Ok,
                                )?,
                            };
                            if v < min || v > max {
                                return fail(
                                    path,
                                    &key,
                                    format!("the level {v} lies outside [{min}, {max}]"),
                                );
                            }
                            if out.contains(&v) {
                                return fail(path, &key, format!("the level {v} is listed twice"));
                            }
                            out.push(v);
                        }
                        Some(out)
                    }
                };
                if grid && levels.is_none() {
                    return fail(
                        path,
                        &key,
                        "a grid search needs `levels` on every range (HYP-6)".into(),
                    );
                }
                if int {
                    #[allow(clippy::cast_possible_truncation)]
                    let i = |f: f64| f as i64;
                    Domain::IntRange {
                        min: i(min),
                        max: i(max),
                        levels: levels.map(|v| v.into_iter().map(i).collect()),
                    }
                } else {
                    Domain::Range { min, max, levels }
                }
            }
            k => {
                return fail(
                    path,
                    &key,
                    format!("kind `{k}` is not bool, enum, range or int_range"),
                );
            }
        };
        if p.pooled == Some(false)
            && !matches!(domain, Domain::Bool | Domain::Enum(_))
            && !levels_given
        {
            return fail(path,
                &key,
                "`pooled = false` needs a bool, an enum or `levels`, so the slices are a finite, declared set".into(),
            );
        }
        // CON-26: `provider` is never pooled.
        let pooled = name != "provider" && p.pooled != Some(false);
        params.insert(
            name.clone(),
            Param {
                name: name.clone(),
                domain,
                pooled,
            },
        );
    }

    // HYP-7: [measures].
    let primary = raw.measures.primary.clone();
    let secondary = raw.measures.secondary.clone().unwrap_or_default();
    if primary.is_empty() {
        return fail(
            path,
            "measures.primary",
            "lists at least one quantity".into(),
        );
    }
    let mut measured = BTreeSet::new();
    for q in primary.iter().chain(&secondary) {
        if is_reserved(q) {
            return fail(path, "measures", format!("`{q}` is a reserved word"));
        }
        if params.contains_key(q) {
            return fail(
                path,
                "measures",
                format!("`{q}` is a parameter as well as a quantity"),
            );
        }
        if !measured.insert(q.clone()) {
            return fail(path, "measures", format!("`{q}` is listed twice"));
        }
        if crate::quantities::get(q).is_none() {
            if frozen {
                return fail(
                    path,
                    "measures",
                    format!("`{q}` resolves to no quantity of HYP-12's table"),
                );
            }
            warnings.push(format!(
                "`{q}` is not in the quantity table: a verdict that needs it is inconclusive (HYP-7)"
            ));
        }
    }

    // HYP-8: [control].
    let c = &raw.control;
    let control = match (&c.config, &c.workload) {
        (Some(_), Some(_)) => {
            return fail(
                path,
                "control",
                "carries `config` or `workload`, not both".into(),
            );
        }
        (Some(cfg), None) => {
            if c.inherits.is_some() {
                return fail(
                    path,
                    "control.inherits",
                    "belongs to a `workload` control".into(),
                );
            }
            let mut out = BTreeMap::new();
            for (k, v) in cfg {
                let key = format!("control.config.{k}");
                let Some(p) = params.get(k) else {
                    return fail(path, &key, format!("`{k}` is not a [varies] parameter"));
                };
                if !p.pooled {
                    return fail(
                        path,
                        &key,
                        format!("`{k}` is not pooled; a control assigns pooled parameters"),
                    );
                }
                let lit = match v {
                    toml::Value::Boolean(b) => predicate::Lit::Bool(*b),
                    toml::Value::String(s) => predicate::Lit::Ident(s.clone()),
                    v => match number(v) {
                        Some(n) => predicate::Lit::Num(n),
                        None => return fail(path, &key, "not a value of the parameter".into()),
                    },
                };
                if !p.domain.admits(&lit) {
                    return fail(
                        path,
                        &key,
                        format!("`{v}` lies outside the domain of `{k}`"),
                    );
                }
                out.insert(k.clone(), lit);
            }
            Control::Config(out)
        }
        (None, Some(mode)) => {
            if let Some(inh) = &c.inherits {
                for k in inh {
                    if !params.contains_key(k) {
                        return fail(
                            path,
                            "control.inherits",
                            format!("`{k}` is not a [varies] parameter"),
                        );
                    }
                }
            }
            Control::Workload {
                mode: mode.clone(),
                inherits: c.inherits.clone(),
            }
        }
        (None, None) => {
            if frozen {
                return fail(
                    path,
                    "control",
                    "a frozen file carries `config` or `workload` (HYP-8, CON-18)".into(),
                );
            }
            warnings.push("no control: every verdict is inconclusive (HYP-8, CON-18)".into());
            Control::Missing
        }
    };

    // HYP-9: [design].
    let d = &raw.design;
    if !matches!(d.search.as_str(), "grid" | "bisect" | "random") {
        return fail(
            path,
            "design.search",
            format!("`{}` is not grid, bisect or random", d.search),
        );
    }
    if frozen && d.search != "grid" {
        return fail(
            path,
            "design.search",
            "a frozen file uses `grid` (HYP-9)".into(),
        );
    }
    let min_reps = if frozen { 20 } else { 4 };
    if d.replicates < min_reps || d.replicates % 2 != 0 {
        return fail(
            path,
            "design.replicates",
            format!(
                "an even integer of at least {min_reps}, not {}",
                d.replicates
            ),
        );
    }
    let replicates = u32::try_from(d.replicates)
        .map_err(|_| HypError::new(path, Some("design.replicates".into()), "too large".into()))?;
    if let Some(s) = &d.seeds
        && s != "derived"
    {
        return fail(
            path,
            "design.seeds",
            format!("only `derived` is accepted, not `{s}`"),
        );
    }
    let seed = match d.seed {
        Some(_) if frozen => {
            return fail(
                path,
                "design.seed",
                "a frozen file's seed is derived from its hash (HYP-9)".into(),
            );
        }
        Some(s) => Some(u64::try_from(s).map_err(|_| {
            HypError::new(
                path,
                Some("design.seed".into()),
                "from 0 to 2^63 - 1".into(),
            )
        })?),
        None => None,
    };
    let backends = d.backends.clone().unwrap_or_default();
    for b in &backends {
        if b != "mockllm" && b != "real-api" {
            return fail(
                path,
                "design.backends",
                format!("`{b}` is not mockllm or real-api"),
            );
        }
    }
    let provider_count = params.get("provider").and_then(|p| match &p.domain {
        Domain::Enum(v) => Some(v.len()),
        _ => None,
    });
    let min_providers_for_verdict = match d.min_providers_for_verdict {
        None => None,
        Some(m) => {
            let Some(n) = provider_count else {
                return fail(
                    path,
                    "design.min_providers_for_verdict",
                    "needs an enum `provider` parameter".into(),
                );
            };
            if m < 1 || m as usize > n {
                return fail(
                    path,
                    "design.min_providers_for_verdict",
                    format!("lies between 1 and the {n} declared providers"),
                );
            }
            Some(m as u32)
        }
    };
    let mut sim_live_tolerance = BTreeMap::new();
    for (q, v) in d.sim_live_tolerance.iter().flatten() {
        let key = format!("design.sim_live_tolerance.{q}");
        if !measured.contains(q) {
            return fail(path, &key, format!("`{q}` is not a [measures] quantity"));
        }
        let t = match v {
            toml::Value::Table(t) => {
                let abs = t.get("abs").and_then(number);
                if t.len() != 1 || abs.is_none() {
                    return fail(path, &key, "a table tolerance is `{ abs = x }`".into());
                }
                Tolerance::Absolute(abs.unwrap_or_default())
            }
            v => match number(v) {
                Some(x) if x <= 0.5 => Tolerance::Relative(x),
                Some(_) => return fail(path, &key, "a relative tolerance is at most 0.5".into()),
                None => return fail(path, &key, "a number or `{ abs = x }`".into()),
            },
        };
        let x = match t {
            Tolerance::Relative(x) | Tolerance::Absolute(x) => x,
        };
        if !(x.is_finite() && x > 0.0) {
            return fail(
                path,
                &key,
                "a tolerance is finite and greater than zero".into(),
            );
        }
        sim_live_tolerance.insert(q.clone(), t);
    }
    let pins = match &d.pins {
        None => None,
        Some(p) => {
            for (k, list) in [("scenario", &p.scenario), ("workload", &p.workload)] {
                if let Some(h) = list.iter().find(|h| !is_hex_digest(h)) {
                    return fail(
                        path,
                        &format!("design.pins.{k}"),
                        format!("`{h}` is not a BLAKE3 hex digest"),
                    );
                }
            }
            if let Some(Param {
                domain: Domain::Enum(providers),
                ..
            }) = params.get("provider")
                && let Some(k) = p.models.keys().find(|k| !providers.contains(k))
            {
                return fail(
                    path,
                    "design.pins.models",
                    format!("`{k}` is not a provider value"),
                );
            }
            Some(Pins {
                scenario: p.scenario.clone(),
                workload: p.workload.clone(),
                models: p.models.clone(),
            })
        }
    };

    // HYP-9: [expected].
    if !matches!(raw.expected.outcome.as_str(), "pass" | "fail") {
        return fail(path, "expected.outcome", "pass or fail".into());
    }

    // HYP-10..14: the predicates, typed against the file.
    let predicate = predicate::parse(&raw.falsifier.predicate)
        .map_err(|e| HypError::new(path, Some("falsifier.predicate".into()), e.to_string()))?;
    let guard = match &raw.falsifier.inconclusive_if {
        None => None,
        Some(g) => Some(predicate::parse(g).map_err(|e| {
            HypError::new(
                path,
                Some("falsifier.inconclusive_if".into()),
                e.to_string(),
            )
        })?),
    };

    let h = Hypothesis {
        path: loc.path.clone(),
        hash: Digest::of(bytes),
        status: loc.status,
        id: poc.id.clone(),
        title: poc.title.clone(),
        spec: poc.spec.clone(),
        report_refs: poc.report_refs.clone().unwrap_or_default(),
        supersedes: poc.supersedes.clone(),
        statement: raw.hypothesis.statement.clone(),
        params,
        primary,
        secondary,
        control_description: c.description.clone(),
        control,
        design: Design {
            search: d.search.clone(),
            replicates,
            twin_required: d.twin_required,
            seed,
            backends,
            min_providers_for_verdict,
            sim_live_tolerance,
            pins,
        },
        predicate,
        guard,
        expected_outcome: raw.expected.outcome.clone(),
        expected_note: raw.expected.note.clone(),
        warnings,
    };
    crate::check::check(h)
}

/// HYP-2: an `id` is unique across `hypotheses/`.
fn unique_id(root: &Path, path: &Path, id: &str) -> Result<(), HypError> {
    let dir = root.join("hypotheses");
    let Ok(entries) = std::fs::read_dir(&dir) else {
        return Ok(());
    };
    let me = std::fs::canonicalize(path).unwrap_or_else(|_| path.to_path_buf());
    for e in entries.flatten() {
        let p = e.path();
        if p.extension().is_none_or(|x| x != "toml")
            || std::fs::canonicalize(&p).unwrap_or_else(|_| p.clone()) == me
        {
            continue;
        }
        let other = std::fs::read_to_string(&p)
            .ok()
            .and_then(|t| toml::from_str::<toml::Table>(&t).ok())
            .and_then(|t| t.get("poc")?.get("id")?.as_str().map(str::to_owned));
        if other.as_deref() == Some(id) {
            return Err(HypError::new(
                path,
                Some("poc.id".into()),
                format!("`{id}` is also the id of {}", p.display()),
            ));
        }
    }
    Ok(())
}
