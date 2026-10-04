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

    /// Whether `v` is a value a run can take: in the domain, and one of the
    /// levels when the parameter declares levels (a grid runs nothing else).
    #[must_use]
    pub fn admits(&self, v: &predicate::Lit) -> bool {
        use predicate::Lit;
        match (self, v) {
            (Self::Bool, Lit::Bool(_)) => true,
            (Self::Enum(vals), Lit::Ident(s)) => vals.contains(s),
            (Self::Range { min, max, levels }, Lit::Num(n)) => match levels {
                Some(ls) => ls.contains(n),
                None => *min <= *n && *n <= *max,
            },
            (Self::IntRange { min, max, levels }, Lit::Num(n)) => {
                as_int(*n).is_some_and(|i| match levels {
                    Some(ls) => ls.contains(&i),
                    None => *min <= i && i <= *max,
                })
            }
            _ => false,
        }
    }

    /// Whether some value a run can take satisfies `x op bound` (an `at` bound
    /// that no cell satisfies makes the predicate vacuous, ADR-18).
    #[must_use]
    pub fn satisfiable(&self, op: predicate::CmpOp, bound: f64) -> bool {
        use predicate::CmpOp;
        let holds = |x: f64| op.apply(x, bound);
        match self {
            Self::Range {
                levels: Some(ls), ..
            } => ls.iter().any(|x| holds(*x)),
            Self::IntRange {
                levels: Some(ls), ..
            } => ls.iter().any(|x| holds(*x as f64)),
            Self::Range { min, max, .. } => match op {
                CmpOp::Lt => *min < bound,
                CmpOp::Le => *min <= bound,
                CmpOp::Gt => *max > bound,
                CmpOp::Ge => *max >= bound,
                CmpOp::Eq => *min <= bound && bound <= *max,
                CmpOp::Ne => !(*min == bound && *max == bound),
            },
            Self::IntRange { min, max, .. } => {
                let (lo, hi) = (*min as f64, *max as f64);
                match op {
                    CmpOp::Lt => lo < bound,
                    CmpOp::Le => lo <= bound,
                    CmpOp::Gt => hi > bound,
                    CmpOp::Ge => hi >= bound,
                    CmpOp::Eq => as_int(bound).is_some_and(|b| *min <= b && b <= *max),
                    CmpOp::Ne => !(min == max && as_int(bound) == Some(*min)),
                }
            }
            Self::Bool | Self::Enum(_) => false,
        }
    }

    /// Whether the domain is numeric (it can be bounded by an `at` clause).
    #[must_use]
    pub fn is_numeric(&self) -> bool {
        matches!(self, Self::Range { .. } | Self::IntRange { .. })
    }
}

/// `f` as an exact integer, if it is one in `i64`'s range.
pub(crate) fn as_int(f: f64) -> Option<i64> {
    #[allow(clippy::cast_possible_truncation)]
    let i = f as i64;
    (f.fract() == 0.0 && (i as f64) == f && f.abs() < 9.2e18).then_some(i)
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

/// A loaded, checked hypothesis file. Only [`load`] and [`load_in`] make one:
/// its status, hash and checked predicates are private, so no caller can mark a
/// file frozen or swap a predicate after the checks (CON-7, HYP-3).
#[derive(Debug, Clone)]
pub struct Hypothesis {
    pub path: PathBuf,
    hash: Digest,
    status: Status,
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
    predicate: Expr,
    guard: Option<Expr>,
    pub expected_outcome: String,
    pub expected_note: Option<String>,
    /// What lint and load report without failing (HYP-7, HYP-8).
    pub warnings: Vec<String>,
}

/// The error a loop or a verdict aborts with when a hypothesis file changed
/// under it (HYP-4, LOOP-13).
pub const HYPOTHESIS_CHANGED: &str = "hypothesis_changed";

impl Hypothesis {
    /// HYP-4: read the file again, read-only, and fail with
    /// [`HYPOTHESIS_CHANGED`] if its bytes no longer have the hash it was loaded
    /// with (or it cannot be read). A loop calls this before every step it takes
    /// on the file's behalf (LOOP-13), and `acn hyp verdict` before it writes.
    pub fn check_unchanged(&self) -> Result<(), HypError> {
        let now = std::fs::read(&self.path).map(|b| Digest::of(&b));
        match now {
            Ok(d) if d == self.hash => Ok(()),
            Ok(d) => Err(HypError::new(
                &self.path,
                None,
                format!(
                    "{HYPOTHESIS_CHANGED}: the file's hash is now {} where it was {} (HYP-4)",
                    d.to_hex(),
                    self.hash.to_hex()
                ),
            )),
            Err(e) => Err(HypError::new(
                &self.path,
                None,
                format!("{HYPOTHESIS_CHANGED}: the file can no longer be read: {e} (HYP-4)"),
            )),
        }
    }

    /// BLAKE3 of the file's bytes (HYP-5).
    #[must_use]
    pub fn hash(&self) -> Digest {
        self.hash
    }

    /// Frozen or candidate, by location and record (HYP-3).
    #[must_use]
    pub fn status(&self) -> Status {
        self.status
    }

    /// The falsifier, checked, with its selectors normalised to `pname = value`.
    #[must_use]
    pub fn predicate(&self) -> &Expr {
        &self.predicate
    }

    /// The guard `inconclusive_if`, checked.
    #[must_use]
    pub fn guard(&self) -> Option<&Expr> {
        self.guard.as_ref()
    }

    /// Every quantity `[measures]` lists.
    pub fn measures(&self) -> impl Iterator<Item = &String> {
        self.primary.iter().chain(&self.secondary)
    }

    /// Whether the file has a `provider` parameter.
    #[must_use]
    pub fn has_provider(&self) -> bool {
        self.params.contains_key("provider")
    }

    /// The number of declared providers, when `provider` is an enum.
    #[must_use]
    pub fn provider_count(&self) -> Option<usize> {
        match &self.params.get("provider")?.domain {
            Domain::Enum(v) => Some(v.len()),
            _ => None,
        }
    }

    /// The parameter that declares enum value `v` (values are unique, HYP-6).
    #[must_use]
    pub fn param_of_value(&self, v: &str) -> Option<&Param> {
        self.params
            .values()
            .find(|p| matches!(&p.domain, Domain::Enum(vals) if vals.iter().any(|x| x == v)))
    }

    pub(crate) fn set_predicates(&mut self, predicate: Expr, guard: Option<Expr>) {
        self.predicate = predicate;
        self.guard = guard;
    }
}

// ---- loading ---------------------------------------------------------------

/// Where a file was found, which decides its status (HYP-3).
#[derive(Debug, Clone)]
pub(crate) struct Location {
    pub path: PathBuf,
    pub status: Status,
    /// The workspace root (CON-28), when there is one.
    pub root: Option<PathBuf>,
    /// Why a file under `hypotheses/` is nonetheless a candidate.
    pub note: Option<String>,
}

fn io(path: &Path) -> impl Fn(std::io::Error) -> HypError + '_ {
    move |e| HypError::new(path, None, e.to_string())
}

fn env_err(path: &Path) -> impl Fn(acn_trace::env::EnvError) -> HypError + '_ {
    move |e| HypError::new(path, None, e.to_string())
}

/// HYP-3: frozen when the file lies under `<root>/hypotheses/` and `env-hash.json`
/// lists that path with this hash. The root is CON-28's: found from `start`, the
/// current directory, never from the file. A record that does not match the
/// frozen set freezes nothing: every file is then a candidate, as a locally
/// edited copy is (HYP-3, CON-28).
fn locate(path: &Path, hash: &Digest, start: &Path) -> Result<Location, HypError> {
    let abs = std::fs::canonicalize(path).map_err(io(path))?;
    let root = acn_trace::env::find_root(start).map_err(env_err(path))?;
    let mut status = Status::Candidate;
    let mut note = None;
    if let Some(root) = &root
        && abs.starts_with(root.join("hypotheses"))
        && let Some(record) = acn_trace::env::read_record(root).map_err(env_err(path))?
    {
        let computed = acn_trace::env::compute(root).map_err(env_err(path))?;
        let rel = acn_trace::env::rel_path(root, &abs).map_err(env_err(path))?;
        let listed = record
            .files
            .iter()
            .any(|f| f.path == rel && f.blake3 == hash.to_hex());
        if computed.env_hash != record.env_hash
            || computed.engine_hash != record.engine_hash
            || computed.files != record.files
        {
            note = Some(format!(
                "{} does not match the frozen set under {}: nothing there is frozen until `cargo xtask env-hash --write` records it (CON-28)",
                acn_trace::env::RECORD_FILE,
                root.display()
            ));
        } else if listed {
            status = Status::Frozen;
        } else {
            note = Some(format!(
                "{rel} is not recorded in {} (HYP-3)",
                acn_trace::env::RECORD_FILE
            ));
        }
    }
    Ok(Location {
        path: abs,
        status,
        root,
        note,
    })
}

/// Load a hypothesis file read-only (HYP-4) and check it (HYP-1..14), its status
/// decided against the workspace root of the current directory (CON-28).
pub fn load(path: &Path) -> Result<Hypothesis, HypError> {
    let cwd = std::env::current_dir().map_err(io(path))?;
    load_in(path, &cwd)
}

/// [`load`], with the workspace root found from `start` instead of the current
/// directory.
pub fn load_in(path: &Path, start: &Path) -> Result<Hypothesis, HypError> {
    let bytes = std::fs::read(path).map_err(io(path))?;
    let loc = locate(path, &Digest::of(&bytes), start)?;
    parse(&bytes, &loc)
}

fn fail<T>(path: &Path, key: &str, m: String) -> Result<T, HypError> {
    Err(HypError::new(path, Some(key.to_owned()), m))
}

fn finite_number(v: &toml::Value) -> Option<f64> {
    match v {
        toml::Value::Float(f) if f.is_finite() => Some(*f),
        #[allow(clippy::cast_precision_loss)]
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
pub(crate) fn parse(bytes: &[u8], loc: &Location) -> Result<Hypothesis, HypError> {
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
    let mut warnings: Vec<String> = loc.note.iter().cloned().collect();
    check_poc(&raw.poc, loc)?;
    let grid = raw.design.search == "grid";
    let params = check_varies(path, &raw.varies, grid)?;
    let (primary, secondary) = check_measures(path, &raw.measures, &params, frozen, &mut warnings)?;
    let measured: BTreeSet<String> = primary.iter().chain(&secondary).cloned().collect();
    let control = check_control(path, &raw.control, &params, frozen, &mut warnings)?;
    let design = check_design(path, &raw.design, &params, &measured, frozen)?;
    if !matches!(raw.expected.outcome.as_str(), "pass" | "fail") {
        return fail(path, "expected.outcome", "pass or fail".into());
    }
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
        id: raw.poc.id.clone(),
        title: raw.poc.title.clone(),
        spec: raw.poc.spec.clone(),
        report_refs: raw.poc.report_refs.clone().unwrap_or_default(),
        supersedes: raw.poc.supersedes.clone(),
        statement: raw.hypothesis.statement.clone(),
        params,
        primary,
        secondary,
        control_description: raw.control.description.clone(),
        control,
        design,
        predicate,
        guard,
        expected_outcome: raw.expected.outcome.clone(),
        expected_note: raw.expected.note.clone(),
        warnings,
    };
    crate::check::check(h)
}

/// HYP-2: `[poc]`, the stem rule, `status`, `supersedes`, and a frozen file's spec
/// and unique id.
fn check_poc(poc: &RawPoc, loc: &Location) -> Result<(), HypError> {
    let path = loc.path.as_path();
    if !predicate::is_ident(&poc.id) {
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
    if loc.status == Status::Frozen {
        let Some(spec) = &poc.spec else {
            return fail(path, "poc.spec", "a frozen file names its POC spec".into());
        };
        // `specs/<name>.md`: one component, no `..`, nothing absolute.
        let name = spec.strip_prefix("specs/").filter(|n| {
            n.ends_with(".md") && n.len() > 3 && !n.contains(['/', '\\']) && !n.starts_with('.')
        });
        let Some(name) = name else {
            return fail(
                path,
                "poc.spec",
                format!("`{spec}` is not `specs/<name>.md`"),
            );
        };
        if let Some(root) = &loc.root {
            let exists = root.join(spec).is_file();
            let listed =
                std::fs::read_to_string(root.join("specs/README.md")).is_ok_and(|readme| {
                    readme
                        .lines()
                        .flat_map(|l| l.split('|'))
                        .any(|cell| cell.trim() == name)
                });
            if !exists && !listed {
                return fail(
                    path,
                    "poc.spec",
                    format!("`{spec}` neither exists nor is listed in specs/README.md"),
                );
            }
            unique_id(root, path, &poc.id)?;
        }
    }
    Ok(())
}

/// HYP-6: `[varies]`.
fn check_varies(
    path: &Path,
    raw: &BTreeMap<String, RawParam>,
    grid: bool,
) -> Result<BTreeMap<String, Param>, HypError> {
    let mut params = BTreeMap::new();
    let mut values_seen: BTreeMap<String, String> = BTreeMap::new();
    for (name, p) in raw {
        let key = format!("varies.{name}");
        if is_reserved(name) {
            return fail(path, &key, format!("`{name}` is a reserved word (HYP-10)"));
        }
        let domain = check_param(path, &key, name, p, grid, &mut values_seen)?;
        let finite = domain.size().is_some();
        if (p.pooled == Some(false) || name == "provider") && !finite {
            return fail(
                path,
                &key,
                "a non-pooled parameter needs a bool, an enum or `levels`, so the slices are a finite, declared set".into(),
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
    Ok(params)
}

fn check_param(
    path: &Path,
    key: &str,
    name: &str,
    p: &RawParam,
    grid: bool,
    values_seen: &mut BTreeMap<String, String>,
) -> Result<Domain, HypError> {
    let none_of = |fields: &[(&str, bool)]| -> Result<(), HypError> {
        for (f, present) in fields {
            if *present {
                return fail(
                    path,
                    key,
                    format!("a `{}` parameter takes no `{f}`", p.kind),
                );
            }
        }
        Ok(())
    };
    match p.kind.as_str() {
        "bool" => {
            none_of(&[
                ("values", p.values.is_some()),
                ("min", p.min.is_some()),
                ("max", p.max.is_some()),
                ("levels", p.levels.is_some()),
            ])?;
            Ok(Domain::Bool)
        }
        "enum" => {
            none_of(&[
                ("min", p.min.is_some()),
                ("max", p.max.is_some()),
                ("levels", p.levels.is_some()),
            ])?;
            let Some(values) = p.values.clone().filter(|v| !v.is_empty()) else {
                return fail(path, key, "an enum lists its `values`, at least one".into());
            };
            for v in &values {
                if is_reserved(v) {
                    return fail(path, key, format!("the value `{v}` is a reserved word"));
                }
                if let Some(other) = values_seen.insert(v.clone(), name.to_owned()) {
                    return fail(
                        path,
                        key,
                        format!(
                            "the value `{v}` is also a value of `{other}`; enum values are unique across parameters"
                        ),
                    );
                }
            }
            Ok(Domain::Enum(values))
        }
        "range" => {
            none_of(&[("values", p.values.is_some())])?;
            let read = |v: &Option<toml::Value>, f: &str| -> Result<f64, HypError> {
                v.as_ref().and_then(finite_number).map_or_else(
                    || fail(path, key, format!("`{f}` is not a finite number")),
                    Ok,
                )
            };
            let (min, max) = (read(&p.min, "min")?, read(&p.max, "max")?);
            if min > max {
                return fail(path, key, format!("min {min} exceeds max {max}"));
            }
            let levels = match &p.levels {
                None => None,
                Some(ls) => {
                    let mut out: Vec<f64> = Vec::new();
                    for l in ls {
                        let v = finite_number(l).map_or_else(
                            || fail(path, key, "a level is not a finite number".into()),
                            Ok,
                        )?;
                        level_ok(
                            path,
                            key,
                            v >= min && v <= max,
                            out.contains(&v),
                            &format!("{v}"),
                        )?;
                        out.push(v);
                    }
                    Some(nonempty(path, key, out)?)
                }
            };
            if grid && levels.is_none() {
                return fail(
                    path,
                    key,
                    "a grid search needs `levels` on every range (HYP-6)".into(),
                );
            }
            Ok(Domain::Range { min, max, levels })
        }
        "int_range" => {
            none_of(&[("values", p.values.is_some())])?;
            let read = |v: &Option<toml::Value>, f: &str| -> Result<i64, HypError> {
                match v {
                    Some(toml::Value::Integer(i)) => Ok(*i),
                    _ => fail(path, key, format!("`{f}` of an int_range is an integer")),
                }
            };
            let (min, max) = (read(&p.min, "min")?, read(&p.max, "max")?);
            if min > max {
                return fail(path, key, format!("min {min} exceeds max {max}"));
            }
            let levels = match &p.levels {
                None => None,
                Some(ls) => {
                    let mut out: Vec<i64> = Vec::new();
                    for l in ls {
                        let toml::Value::Integer(v) = l else {
                            return fail(path, key, "an int_range's levels are integers".into());
                        };
                        level_ok(
                            path,
                            key,
                            *v >= min && *v <= max,
                            out.contains(v),
                            &format!("{v}"),
                        )?;
                        out.push(*v);
                    }
                    Some(nonempty(path, key, out)?)
                }
            };
            if grid && levels.is_none() {
                return fail(
                    path,
                    key,
                    "a grid search needs `levels` on every range (HYP-6)".into(),
                );
            }
            Ok(Domain::IntRange { min, max, levels })
        }
        k => fail(
            path,
            key,
            format!("kind `{k}` is not bool, enum, range or int_range"),
        ),
    }
}

fn level_ok(path: &Path, key: &str, inside: bool, twice: bool, v: &str) -> Result<(), HypError> {
    if !inside {
        return fail(path, key, format!("the level {v} lies outside [min, max]"));
    }
    if twice {
        return fail(path, key, format!("the level {v} is listed twice"));
    }
    Ok(())
}

fn nonempty<T>(path: &Path, key: &str, v: Vec<T>) -> Result<Vec<T>, HypError> {
    if v.is_empty() {
        return fail(path, key, "`levels` is empty".into());
    }
    Ok(v)
}

/// HYP-7: `[measures]`.
fn check_measures(
    path: &Path,
    m: &RawMeasures,
    params: &BTreeMap<String, Param>,
    frozen: bool,
    warnings: &mut Vec<String>,
) -> Result<(Vec<String>, Vec<String>), HypError> {
    let primary = m.primary.clone();
    let secondary = m.secondary.clone().unwrap_or_default();
    if primary.is_empty() {
        return fail(
            path,
            "measures.primary",
            "lists at least one quantity".into(),
        );
    }
    let mut seen = BTreeSet::new();
    for q in primary.iter().chain(&secondary) {
        if is_reserved(q) || !predicate::is_ident(q) {
            return fail(
                path,
                "measures",
                format!("`{q}` is a reserved word or not an identifier"),
            );
        }
        if params.contains_key(q) {
            return fail(
                path,
                "measures",
                format!("`{q}` is a parameter as well as a quantity"),
            );
        }
        if !seen.insert(q.clone()) {
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
    Ok((primary, secondary))
}

/// HYP-8: `[control]`.
fn check_control(
    path: &Path,
    c: &RawControl,
    params: &BTreeMap<String, Param>,
    frozen: bool,
    warnings: &mut Vec<String>,
) -> Result<Control, HypError> {
    match (&c.config, &c.workload) {
        (Some(_), Some(_)) => fail(
            path,
            "control",
            "carries `config` or `workload`, not both".into(),
        ),
        (Some(cfg), None) => {
            if c.inherits.is_some() {
                return fail(
                    path,
                    "control.inherits",
                    "belongs to a `workload` control".into(),
                );
            }
            if cfg.is_empty() {
                return fail(
                    path,
                    "control.config",
                    "assigns at least one parameter; an empty config is the treatment itself"
                        .into(),
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
                    v => match finite_number(v) {
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
            Ok(Control::Config(out))
        }
        (None, Some(mode)) => {
            for k in c.inherits.iter().flatten() {
                if !params.contains_key(k) {
                    return fail(
                        path,
                        "control.inherits",
                        format!("`{k}` is not a [varies] parameter"),
                    );
                }
            }
            Ok(Control::Workload {
                mode: mode.clone(),
                inherits: c.inherits.clone(),
            })
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
            Ok(Control::Missing)
        }
    }
}

/// HYP-9: `[design]`.
fn check_design(
    path: &Path,
    d: &RawDesign,
    params: &BTreeMap<String, Param>,
    measured: &BTreeSet<String>,
    frozen: bool,
) -> Result<Design, HypError> {
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
    let providers: Option<&Vec<String>> = params.get("provider").and_then(|p| match &p.domain {
        Domain::Enum(v) => Some(v),
        _ => None,
    });
    let min_providers_for_verdict = match d.min_providers_for_verdict {
        None => None,
        Some(m) => {
            let Some(n) = providers.map(Vec::len) else {
                return fail(
                    path,
                    "design.min_providers_for_verdict",
                    "needs an enum `provider` parameter".into(),
                );
            };
            match u32::try_from(m) {
                Ok(m) if m >= 1 && (m as usize) <= n => Some(m),
                _ => {
                    return fail(
                        path,
                        "design.min_providers_for_verdict",
                        format!("lies between 1 and the {n} declared providers"),
                    );
                }
            }
        }
    };
    let mut sim_live_tolerance = BTreeMap::new();
    for (q, v) in d.sim_live_tolerance.iter().flatten() {
        let key = format!("design.sim_live_tolerance.{q}");
        if !measured.contains(q) {
            return fail(path, &key, format!("`{q}` is not a [measures] quantity"));
        }
        let t = match v {
            toml::Value::Table(t) => match (t.len(), t.get("abs").and_then(finite_number)) {
                (1, Some(abs)) => Tolerance::Absolute(abs),
                _ => {
                    return fail(
                        path,
                        &key,
                        "a table tolerance is exactly `{ abs = x }`".into(),
                    );
                }
            },
            v => match finite_number(v) {
                Some(x) if x <= 0.5 => Tolerance::Relative(x),
                Some(_) => return fail(path, &key, "a relative tolerance is at most 0.5".into()),
                None => return fail(path, &key, "a number or `{ abs = x }`".into()),
            },
        };
        let (Tolerance::Relative(x) | Tolerance::Absolute(x)) = t;
        if x <= 0.0 {
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
            if let Some(k) = p
                .models
                .keys()
                .find(|k| providers.is_none_or(|ps| !ps.contains(k)))
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
    Ok(Design {
        search: d.search.clone(),
        replicates,
        twin_required: d.twin_required,
        seed,
        backends,
        min_providers_for_verdict,
        sim_live_tolerance,
        pins,
    })
}

/// HYP-2: an `id` is unique across `hypotheses/`, subdirectories included. A file
/// there that cannot be read or parsed is an error: it might hold the same id.
fn unique_id(root: &Path, path: &Path, id: &str) -> Result<(), HypError> {
    let dir = root.join("hypotheses");
    let me = std::fs::canonicalize(path).map_err(io(path))?;
    let mut stack = vec![dir];
    while let Some(d) = stack.pop() {
        for e in std::fs::read_dir(&d).map_err(io(&d))? {
            let p = e.map_err(io(&d))?.path();
            if p.is_dir() {
                stack.push(p);
                continue;
            }
            if p.extension().is_none_or(|x| x != "toml")
                || std::fs::canonicalize(&p).map_err(io(&p))? == me
            {
                continue;
            }
            let text = std::fs::read_to_string(&p).map_err(io(&p))?;
            let table: toml::Table = toml::from_str(&text).map_err(|e| {
                HypError::new(
                    path,
                    Some("poc.id".into()),
                    format!(
                        "cannot check that `{id}` is unique: {} does not parse: {e}",
                        p.display()
                    ),
                )
            })?;
            let other = table
                .get("poc")
                .and_then(|t| t.get("id"))
                .and_then(toml::Value::as_str);
            if other == Some(id) {
                return fail(
                    path,
                    "poc.id",
                    format!("`{id}` is also the id of {}", p.display()),
                );
            }
        }
    }
    Ok(())
}
