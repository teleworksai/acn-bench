//! The one path from bundles to a verdict (HYP-20..24, HYP-28; CON-17): refuse a
//! set that cannot be judged, assemble each slice's cells and arms from the
//! bundles, apply the twin rule, evaluate the falsifier, label the result, decide
//! the file-level verdict under the guard, and render `verdict.json`.

use std::collections::{BTreeMap, BTreeSet};
use std::path::{Path, PathBuf};

use acn_trace::bundle::MOCK_BACKEND;
use acn_trace::identity::{Digest, Mode, Preimage, derived_seed};

use crate::Status;
use crate::bootstrap::{self, Function};
use crate::eval::{self, CounterKind, Source, Term};
use crate::file::{Control, Domain, Hypothesis, Tolerance};
use crate::json::J;
use crate::predicate::{Arg, AtRange, Builtin, Expr, Lit};
use crate::quantities::{self, Replicate, Session};
use crate::read::BundleData;
use crate::slice::{
    Arm, Cell, CellData, ControlData, ControlKind, Evaluation, Observed, Outcome, SliceData,
    SliceEval, Value, Values, key,
};

/// A set that cannot be judged (HYP-20, HYP-21): nothing is written.
#[derive(Debug, Clone, PartialEq, Eq, thiserror::Error)]
#[error("refused: {0}")]
pub struct Refusal(pub String);

fn refuse<T>(m: impl Into<String>) -> Result<T, Refusal> {
    Err(Refusal(m.into()))
}

/// A verdict (§1).
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum V {
    Pass,
    Fail,
    Inconclusive,
}

impl V {
    #[must_use]
    pub fn as_str(self) -> &'static str {
        match self {
            Self::Pass => "pass",
            Self::Fail => "fail",
            Self::Inconclusive => "inconclusive",
        }
    }
}

/// One cause of HYP-21, with the cells or expressions it refers to.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct Reason {
    pub id: &'static str,
    pub refers: Vec<String>,
}

/// HYP-21's order of reasons.
const ORDER: [&str; 10] = [
    "control_missing",
    "no_evaluated_cell",
    "twin_failed",
    "incomplete_cell",
    "grid_cell_missing",
    "undefined_value",
    "guard",
    "slice_inconclusive",
    "providers_below_minimum",
    "no_conclusive_slice",
];

fn sort_reasons(r: &mut [Reason]) {
    r.sort_by_key(|x| ORDER.iter().position(|o| *o == x.id));
}

/// One slice's verdict.
#[derive(Debug, Clone, PartialEq)]
pub struct SliceVerdict {
    pub key: String,
    pub params: Cell,
    pub verdict: V,
    pub reasons: Vec<Reason>,
    pub labels: BTreeSet<&'static str>,
    /// Whether the set holds any bundle of this slice (HYP-24).
    pub has_bundles: bool,
    /// The slice's data, as the falsifier read it.
    pub data: SliceData,
    pub eval: SliceEval,
}

/// A verdict over a bundle set, and its `verdict.json`.
#[derive(Debug, Clone, PartialEq)]
pub struct Verdict {
    pub verdict_id: Digest,
    pub verdict: V,
    pub reasons: Vec<Reason>,
    pub labels: BTreeSet<&'static str>,
    pub slices: Vec<SliceVerdict>,
    /// Every declared provider value: `pass`, `fail`, `inconclusive` or `not_run`.
    pub providers: BTreeMap<String, &'static str>,
    /// `(run_id, bundle_digest)` of every bundle read, ascending.
    pub bundles: Vec<(Digest, Digest)>,
    pub json: J,
}

impl Verdict {
    /// The bytes of `verdict.json` (HYP-15).
    #[must_use]
    pub fn text(&self) -> String {
        self.json.render()
    }
}

/// HYP-9's run seed of a hypothesis: the derived seed of
/// `blake3("acn-bench/hypothesis_seed/v1\0" ‖ hypothesis_hash)`.
pub fn hypothesis_seed(hash: &Digest) -> Result<u64, Refusal> {
    Ok(derived_seed(
        &Preimage::new("acn-bench/hypothesis_seed/v1")
            .map_err(|e| Refusal(e.to_string()))?
            .digest(hash)
            .finish(),
    ))
}

/// `verdict_id = blake3("acn-bench/verdict_id/v1\0" ‖ hypothesis_hash ‖ pairs)`,
/// each pair the raw `run_id` then `bundle_digest`, ascending (HYP-15).
pub fn verdict_id(
    hypothesis_hash: &Digest,
    bundles: &[(Digest, Digest)],
) -> Result<Digest, Refusal> {
    let mut p = Preimage::new("acn-bench/verdict_id/v1")
        .map_err(|e| Refusal(e.to_string()))?
        .digest(hypothesis_hash);
    for (r, d) in bundles {
        p = p.digest(r).digest(d);
    }
    Ok(p.finish())
}

/// Every value a finite parameter takes, in declaration order.
fn values_of(d: &Domain) -> Option<Vec<Value>> {
    match d {
        Domain::Bool => Some(vec![Value::Bool(false), Value::Bool(true)]),
        Domain::Enum(v) => Some(v.iter().map(|s| Value::Enum(s.clone())).collect()),
        Domain::Range {
            levels: Some(l), ..
        } => l.iter().map(|x| Value::float(*x)).collect(),
        Domain::IntRange {
            levels: Some(l), ..
        } => Some(l.iter().map(|x| Value::Int(*x)).collect()),
        _ => None,
    }
}

/// A `[control].config` literal as a cell value of its domain.
fn lit_value(d: &Domain, l: &Lit) -> Option<Value> {
    match (d, l) {
        (Domain::Bool, Lit::Bool(b)) => Some(Value::Bool(*b)),
        (Domain::Enum(_), Lit::Ident(s)) => Some(Value::Enum(s.clone())),
        (Domain::Range { .. }, Lit::Num(n)) => Value::float(*n),
        (Domain::IntRange { .. }, Lit::Num(n)) => {
            #[allow(clippy::cast_possible_truncation)]
            let i = *n as i64;
            #[allow(clippy::cast_precision_loss)]
            ((i as f64) == *n).then_some(Value::Int(i))
        }
        _ => None,
    }
}

/// Every assignment of `params`' values, in order.
fn product(params: &[(String, Vec<Value>)]) -> Vec<Cell> {
    let mut out = vec![Cell::new()];
    for (name, vals) in params {
        let mut next = Vec::with_capacity(out.len() * vals.len());
        for c in &out {
            for v in vals {
                let mut c = c.clone();
                c.insert(name.clone(), v.clone());
                next.push(c);
            }
        }
        out = next;
    }
    out
}

fn sub(c: &Cell, names: &BTreeSet<String>) -> Cell {
    c.iter()
        .filter(|(k, _)| names.contains(*k))
        .map(|(k, v)| (k.clone(), v.clone()))
        .collect()
}

/// What one bundle contributes, once checked.
struct Info<'a> {
    b: &'a BundleData,
    mode: Mode,
    cell: Cell,
    slice: Cell,
    arms: BTreeSet<String>,
    price_key: String,
}

/// The arms of one mode, keyed by `(slice key, arm, configuration key)`.
#[derive(Default)]
struct Arms {
    arms: BTreeMap<(String, String, String), Arm>,
    /// The scenario and workload hashes of the bundles behind each arm.
    hashes: BTreeMap<(String, String, String), BTreeSet<(String, String)>>,
    configs: BTreeMap<(String, String, String), Cell>,
}

const ROLES: [&str; 2] = ["control", "treatment"];

/// The quantities a predicate reads through `effect`, `rel_effect`, `ci_low` or
/// `ci_high` (HYP-22's effect divergence).
fn effect_quantities(e: &Expr, out: &mut BTreeSet<String>) {
    match e {
        Expr::Call(b, args) => {
            if matches!(
                b,
                Builtin::Effect | Builtin::RelEffect | Builtin::CiLow | Builtin::CiHigh
            ) && let Some(q) = eval::qref(args)
            {
                out.insert(q.to_owned());
            }
            for a in args {
                if let Arg::Expr(x) = a {
                    effect_quantities(x, out);
                }
            }
        }
        Expr::Neg(x) | Expr::Not(x) | Expr::At(x, _, _) => effect_quantities(x, out),
        Expr::Arith(_, a, b) | Expr::Cmp(_, a, b) | Expr::And(a, b) | Expr::Or(a, b) => {
            effect_quantities(a, out);
            effect_quantities(b, out);
        }
        _ => {}
    }
}

/// The guard's counters (HYP-12), file-level.
struct Counters {
    replicates: Option<f64>,
    providers: Option<f64>,
}

impl Source for Counters {
    fn term(&self, t: &Term) -> Option<f64> {
        match t {
            Term::Counter(CounterKind::Replicates) => self.replicates,
            Term::Counter(CounterKind::ProvidersReported) => self.providers,
            _ => None,
        }
    }
    fn over_knobs(&self, _max: bool, _x: &Expr) -> Option<f64> {
        None
    }
    fn quantify(&self, _all: bool, _range: &AtRange, _inner: &Expr) -> Option<bool> {
        None
    }
}

/// Per arm, the mean over the replicate indices both modes completed, of `q`.
fn paired_mean(a: &Arm, b: &Arm, q: &str, idx: &[usize]) -> Option<(f64, f64)> {
    if idx.is_empty() {
        return None;
    }
    let (mut sa, mut sb) = (0.0, 0.0);
    for i in idx {
        sa += a.replicates.get(*i)?.as_ref()?.get(q).copied().flatten()?;
        sb += b.replicates.get(*i)?.as_ref()?.get(q).copied().flatten()?;
    }
    let n = bootstrap::as_f64(idx.len());
    Some((sa / n, sb / n))
}

fn both(a: &Arm, b: &Arm) -> Vec<usize> {
    (0..a.replicates.len().min(b.replicates.len()))
        .filter(|i| a.replicates[*i].is_some() && b.replicates[*i].is_some())
        .collect()
}

fn diverge(sim: f64, live: f64, base: f64, tol: Tolerance) -> Option<f64> {
    match tol {
        Tolerance::Absolute(_) => Some((live - sim).abs()),
        Tolerance::Relative(_) => (base != 0.0).then(|| (live - sim).abs() / base.abs()),
    }
}

fn limit(tol: Tolerance) -> f64 {
    match tol {
        Tolerance::Absolute(x) | Tolerance::Relative(x) => x,
    }
}

/// One cell's twin divergences (HYP-22) for one quantity.
#[derive(Debug, Clone, Copy, PartialEq)]
struct Divergence {
    treatment: Option<Option<f64>>,
    control: Option<Option<f64>>,
    effect: Option<Option<f64>>,
}

impl Divergence {
    /// Within `tol` everywhere it was measured.
    fn within(&self, tol: Tolerance) -> bool {
        [self.treatment, self.control, self.effect]
            .into_iter()
            .flatten()
            .all(|d| d.is_some_and(|d| d <= limit(tol)))
    }

    fn json(&self) -> J {
        let f = |d: Option<Option<f64>>| d.map_or(J::Null, J::num);
        J::obj([
            ("control", f(self.control)),
            ("effect", f(self.effect)),
            ("treatment", f(self.treatment)),
        ])
    }
}

fn reason(id: &'static str, refers: Vec<String>) -> Reason {
    Reason { id, refers }
}

fn reasons_json(r: &[Reason]) -> J {
    J::Arr(
        r.iter()
            .map(|r| {
                J::obj([
                    ("reason", J::str(r.id)),
                    ("refers", J::Arr(r.refers.iter().map(J::str).collect())),
                ])
            })
            .collect(),
    )
}

fn cell_json(c: &Cell) -> J {
    J::obj(c.iter().map(|(k, v)| (k.clone(), J::str(v.text()))))
}

fn labels_json(l: &BTreeSet<&'static str>) -> J {
    J::Arr(l.iter().map(|x| J::str(*x)).collect())
}

/// Judge `bundles` against `h` with the tool's own `engine_hash` (CON-28, CON-31).
pub fn verdict(
    h: &Hypothesis,
    mut bundles: Vec<BundleData>,
    engine_hash: Digest,
) -> Result<Verdict, Refusal> {
    if bundles.is_empty() {
        return refuse("no bundles");
    }
    // HYP-15: ascending (run_id, bundle_digest), whatever the argument order.
    bundles.sort_by(|a, b| (a.run_id.0, a.bundle_digest.0).cmp(&(b.run_id.0, b.bundle_digest.0)));
    let r = usize::try_from(h.design.replicates).map_err(|e| Refusal(e.to_string()))?;
    let hash = h.hash();
    let frozen_seed = hypothesis_seed(&hash)?;
    let non_pooled: BTreeSet<String> = h
        .params
        .iter()
        .filter(|(_, p)| !p.pooled)
        .map(|(n, _)| n.clone())
        .collect();
    let has_provider = h.params.contains_key("provider");

    // ---- each bundle on its own (HYP-20) ----
    let mut infos = Vec::with_capacity(bundles.len());
    let mut methods = BTreeSet::new();
    for b in &bundles {
        let m = &b.manifest;
        let name = &m.run_id;
        if m.hypothesis.hash != hash.to_hex() {
            return refuse(format!(
                "{name}: its hypothesis.hash is not the file's ({})",
                hash.to_hex()
            ));
        }
        if m.engine_hash != engine_hash.to_hex() {
            return refuse(format!(
                "{name}: engine_hash {} is not this tool's {} (CON-28, CON-31)",
                m.engine_hash,
                engine_hash.to_hex()
            ));
        }
        let mode = Mode::parse(&m.mode).map_err(|e| Refusal(format!("{name}: {e}")))?;
        let mut cell = Cell::new();
        for (p, param) in &h.params {
            let Some(text) = m.params.get(&format!("vary.{p}")) else {
                return refuse(format!("{name}: no `vary.{p}` in its params (HYP-6)"));
            };
            let Some(v) = Value::parse(&param.domain, text) else {
                return refuse(format!(
                    "{name}: `vary.{p} = {text}` is not a value of its domain (HYP-6)"
                ));
            };
            cell.insert(p.clone(), v);
        }
        if let Some(extra) = m
            .params
            .keys()
            .filter_map(|k| k.strip_prefix("vary."))
            .find(|p| !h.params.contains_key(*p))
        {
            return refuse(format!(
                "{name}: `vary.{extra}` is not a [varies] parameter"
            ));
        }
        let provider = cell.get("provider").map(Value::text);
        if let Some(p) = &provider
            && m.backend != MOCK_BACKEND
            && *p != m.backend
        {
            return refuse(format!(
                "{name}: vary.provider = {p} but the backend is {} (HYP-20)",
                m.backend
            ));
        }
        let price_key = provider.clone().unwrap_or_else(|| m.backend.clone());
        if let Some(pins) = &h.design.pins {
            let model_ok = pins.models.get(&price_key) == Some(&m.model);
            if !pins.scenario.contains(&m.scenario_hash)
                || !pins.workload.contains(&m.workload_hash)
                || !model_ok
            {
                return refuse(format!(
                    "{name}: its scenario, workload or model is not among [design].pins (HYP-20)"
                ));
            }
        }
        if h.status() == Status::Frozen && m.seed != frozen_seed.to_string() {
            return refuse(format!(
                "{name}: run under seed {} but a frozen hypothesis runs under {frozen_seed} (HYP-9)",
                m.seed
            ));
        }
        methods.extend(b.methods.iter().cloned());
        let arms: BTreeSet<String> = m
            .params
            .get("arms")
            .map(|a| a.split(',').map(str::to_owned).collect())
            .unwrap_or_default();
        if let Some(bad) = b.sessions.iter().find(|s| !arms.contains(&s.role)) {
            return refuse(format!(
                "{name}: a session's role `{}` is not among its arms",
                bad.role
            ));
        }
        infos.push(Info {
            b,
            mode,
            slice: sub(&cell, &non_pooled),
            cell,
            arms,
            price_key,
        });
    }

    // ---- the set (HYP-20, HYP-21) ----
    let mock = infos
        .iter()
        .filter(|i| i.b.manifest.backend == MOCK_BACKEND)
        .count();
    if mock != 0 && mock != infos.len() {
        return refuse("the set mixes mockllm and real backends (HYP-20)");
    }
    if methods.len() > 1 {
        return refuse(format!(
            "the set mixes new_input_tokens_method values {methods:?} (TRC-12)"
        ));
    }
    let modes: BTreeSet<Mode> = infos.iter().map(|i| i.mode).collect();
    let (has_sim, has_live, has_netem) = (
        modes.contains(&Mode::Sim),
        modes.contains(&Mode::Live),
        modes.contains(&Mode::Netem),
    );
    if has_netem && (has_sim || has_live) {
        return refuse(
            "netem bundles are refused beside sim or live ones until the M4 rules are written (HYP-21)",
        );
    }
    let qmode = if has_sim {
        Mode::Sim
    } else if has_live {
        Mode::Live
    } else {
        Mode::Netem
    };
    let twin = has_sim && has_live;
    for mode in &modes {
        let builds: BTreeSet<&str> = infos
            .iter()
            .filter(|i| i.mode == *mode)
            .map(|i| i.b.manifest.build.build_hash.as_str())
            .collect();
        if builds.len() > 1 {
            return refuse(format!(
                "more than one build_hash among the {} bundles (HYP-20, CON-31)",
                mode.as_str()
            ));
        }
    }
    let mut per_slice: BTreeMap<String, BTreeSet<(&str, &str)>> = BTreeMap::new();
    for i in &infos {
        per_slice
            .entry(key(&i.slice))
            .or_default()
            .insert((&i.b.manifest.backend, &i.b.manifest.model));
    }
    if let Some((s, _)) = per_slice.iter().find(|(_, v)| v.len() > 1) {
        return refuse(format!(
            "the bundles of slice `{s}` differ in backend or model (HYP-20)"
        ));
    }

    // ---- the control's configuration (HYP-8) ----
    let workload_inherits: BTreeSet<String> = match &h.control {
        Control::Workload { inherits, .. } => inherits.clone().map_or_else(
            || {
                h.params
                    .iter()
                    .filter(|(_, p)| p.domain.is_numeric())
                    .map(|(n, _)| n.clone())
                    .collect()
            },
            |v| v.into_iter().collect(),
        ),
        _ => BTreeSet::new(),
    };
    let kind = match &h.control {
        Control::Workload { .. } => ControlKind::Workload,
        _ => ControlKind::Config,
    };
    let control_config = |c: &Cell| -> Option<Cell> {
        match &h.control {
            Control::Config(cfg) => {
                let mut e = c.clone();
                for (p, l) in cfg {
                    let d = &h.params.get(p)?.domain;
                    e.insert(p.clone(), lit_value(d, l)?);
                }
                Some(e)
            }
            Control::Workload { .. } => {
                let mut e = sub(c, &workload_inherits);
                e.extend(sub(c, &non_pooled));
                Some(e)
            }
            Control::Missing => None,
        }
    };
    // A control bundle's own key: its effective configuration, or what a
    // workload control inherits.
    let own_config = |c: &Cell| -> Cell {
        if kind == ControlKind::Workload {
            let mut e = sub(c, &workload_inherits);
            e.extend(sub(c, &non_pooled));
            e
        } else {
            c.clone()
        }
    };

    // ---- arms from sessions (HYP-21: indices beyond the design are ignored) ----
    let measures: Vec<String> = h.measures().cloned().collect();
    let mut by_mode: BTreeMap<Mode, Arms> = BTreeMap::new();
    let mut ignored: BTreeMap<String, BTreeSet<i64>> = BTreeMap::new();
    let mut covered: BTreeSet<(String, String, String, Mode, usize)> = BTreeSet::new();
    for i in &infos {
        let b = i.b;
        let mut turns_of: BTreeMap<[u8; 8], Vec<usize>> = BTreeMap::new();
        for (k, t) in b.turns.iter().enumerate() {
            turns_of.entry(t.session_id).or_default().push(k);
        }
        let mut calls_of: BTreeMap<[u8; 8], Vec<usize>> = BTreeMap::new();
        for (k, c) in b.calls.iter().enumerate() {
            calls_of.entry(c.session_id).or_default().push(k);
        }
        for role in ROLES {
            if !i.arms.contains(role) {
                continue;
            }
            let config = if role == "control" {
                own_config(&i.cell)
            } else {
                i.cell.clone()
            };
            let akey = (key(&i.slice), role.to_owned(), key(&config));
            let mut reps: BTreeMap<usize, Replicate> = BTreeMap::new();
            for s in b.sessions.iter().filter(|s| s.role == role) {
                let idx = usize::try_from(s.replicate).ok().filter(|x| *x < r);
                let Some(idx) = idx else {
                    ignored
                        .entry(b.manifest.run_id.clone())
                        .or_default()
                        .insert(s.replicate);
                    continue;
                };
                let rep = reps.entry(idx).or_insert_with(|| Replicate {
                    price_key: i.price_key.clone(),
                    ..Replicate::default()
                });
                rep.sessions.push(Session {
                    session_id: s.session_id,
                });
                for k in turns_of.get(&s.session_id).into_iter().flatten() {
                    rep.turns.push(b.turns[*k].clone());
                }
                for k in calls_of.get(&s.session_id).into_iter().flatten() {
                    rep.calls.push(b.calls[*k].clone());
                }
            }
            let arms = by_mode.entry(i.mode).or_default();
            let arm = arms.arms.entry(akey.clone()).or_insert_with(|| Arm {
                replicates: vec![None; r],
            });
            for (idx, rep) in reps {
                if !covered.insert((akey.0.clone(), key(&i.cell), role.to_owned(), i.mode, idx)) {
                    return refuse(format!(
                        "two bundles cover slice `{}`, cell `{}`, arm {role}, mode {}, replicate {idx} (HYP-20)",
                        akey.0,
                        key(&i.cell),
                        i.mode.as_str()
                    ));
                }
                let values: Values = measures
                    .iter()
                    .map(|q| (q.clone(), quantities::value(q, &rep)))
                    .collect();
                arm.replicates[idx] = Some(values);
            }
            arms.hashes.entry(akey.clone()).or_default().insert((
                b.manifest.scenario_hash.clone(),
                b.manifest.workload_hash.clone(),
            ));
            arms.configs.insert(akey, config);
        }
    }
    // A treatment and the control it maps to, and a sim arm and its live twin,
    // share a scenario, and a workload unless the control is a workload control.
    for (mode, arms) in &by_mode {
        for ((s, role, ck), hs) in &arms.hashes {
            if hs.len() > 1 {
                return refuse(format!(
                    "the {} bundles of slice `{s}`, arm {role}, `{ck}` differ in scenario or workload",
                    mode.as_str()
                ));
            }
            if role == "treatment" {
                let cell = &arms.configs[&(s.clone(), role.clone(), ck.clone())];
                if let Some(cc) = control_config(cell)
                    && let Some(ch) = arms
                        .hashes
                        .get(&(s.clone(), "control".to_owned(), key(&cc)))
                {
                    let same = hs
                        .iter()
                        .zip(ch)
                        .all(|(t, c)| t.0 == c.0 && (kind == ControlKind::Workload || t.1 == c.1));
                    if !same {
                        return refuse(format!(
                            "cell `{ck}` and its control differ in scenario_hash or workload_hash (HYP-20)"
                        ));
                    }
                }
            }
        }
    }
    if twin && let (Some(sim), Some(live)) = (by_mode.get(&Mode::Sim), by_mode.get(&Mode::Live)) {
        for (k, hs) in &live.hashes {
            if let Some(ss) = sim.hashes.get(k)
                && ss != hs
            {
                return refuse(format!(
                    "a sim bundle and its live twin differ in scenario or workload (`{}`, HYP-20)",
                    k.2
                ));
            }
        }
    }

    // ---- slices (HYP-24) ----
    let np: Vec<(String, Vec<Value>)> = h
        .params
        .iter()
        .filter(|(n, _)| non_pooled.contains(*n))
        .map(|(n, p)| (n.clone(), values_of(&p.domain).unwrap_or_default()))
        .collect();
    let pooled: Vec<(String, Vec<Value>)> = h
        .params
        .iter()
        .filter(|(n, _)| !non_pooled.contains(*n))
        .map(|(n, p)| (n.clone(), values_of(&p.domain).unwrap_or_default()))
        .collect();
    let mut declared = product(&np);
    declared.sort_by_key(key);
    let one_slice = declared.len() == 1;
    let grid = h.design.search == "grid";
    let seed = bootstrap::verdict_seed(&hash).map_err(|e| Refusal(e.to_string()))?;
    let tolerances = &h.design.sim_live_tolerance;
    let mut effect_qs = BTreeSet::new();
    effect_quantities(h.predicate(), &mut effect_qs);
    let empty = Arms::default();
    let q_arms = by_mode.get(&qmode).unwrap_or(&empty);
    let live_arms = if twin { by_mode.get(&Mode::Live) } else { None };

    let mut slices = Vec::new();
    let mut twin_only = BTreeSet::new();
    for sl in &declared {
        let sk = key(sl);
        let has_bundles = infos.iter().any(|i| key(&i.slice) == sk);
        // Cells: every grid cell, or the cells the bundles ran.
        let universe: Vec<Cell> = if grid {
            product(&pooled)
                .into_iter()
                .map(|mut c| {
                    c.extend(sl.clone());
                    c
                })
                .collect()
        } else {
            let set: BTreeMap<String, Cell> = q_arms
                .configs
                .iter()
                .filter(|((s, role, _), _)| *s == sk && role == "treatment")
                .map(|((_, _, k), c)| (k.clone(), c.clone()))
                .collect();
            set.into_values().collect()
        };
        let mut controls: Vec<ControlData> = Vec::new();
        let mut control_index: BTreeMap<String, usize> = BTreeMap::new();
        for ((s, role, ck), arm) in &q_arms.arms {
            if *s == sk && role == "control" {
                control_index.insert(ck.clone(), controls.len());
                controls.push(ControlData {
                    kind,
                    config: q_arms.configs[&(s.clone(), role.clone(), ck.clone())].clone(),
                    arm: arm.clone(),
                });
            }
        }
        let cells: Vec<CellData> = universe
            .iter()
            .map(|c| CellData {
                cell: c.clone(),
                treatment: q_arms
                    .arms
                    .get(&(sk.clone(), "treatment".to_owned(), key(c)))
                    .cloned(),
                control: control_config(c).and_then(|cc| control_index.get(&key(&cc)).copied()),
            })
            .collect();
        if let Some(live) = live_arms {
            for (s, role, ck) in live.arms.keys() {
                if *s == sk
                    && role == "treatment"
                    && !q_arms
                        .arms
                        .contains_key(&(s.clone(), role.clone(), ck.clone()))
                {
                    twin_only.insert(ck.clone());
                }
            }
        }
        let slice_key = if one_slice { String::new() } else { sk.clone() };
        let data =
            SliceData::new(h, slice_key, cells, controls).map_err(|e| Refusal(e.to_string()))?;
        let ev = Evaluation::new(h.predicate(), &data, seed).map_err(|e| Refusal(e.to_string()))?;
        let eval = ev.run(h.predicate());

        // ---- the twin rule (HYP-22) ----
        let mut divergences: BTreeMap<usize, BTreeMap<String, Divergence>> = BTreeMap::new();
        let mut twinned: BTreeMap<usize, bool> = BTreeMap::new();
        let mut twin_failed = Vec::new();
        if let Some(live) = live_arms {
            for (ci, cd) in data.cells().iter().enumerate() {
                let lt = live
                    .arms
                    .get(&(sk.clone(), "treatment".to_owned(), key(&cd.cell)));
                let ctrl = cd.control.and_then(|k| data.controls().get(k));
                let lc = ctrl.and_then(|k| {
                    live.arms
                        .get(&(sk.clone(), "control".to_owned(), key(&k.config)))
                });
                let st = cd.treatment.as_ref();
                let sc = ctrl.map(|k| &k.arm);
                let ok_pairs = |s: Option<&Arm>, l: Option<&Arm>| match (s, l) {
                    (Some(s), Some(l)) => both(s, l).len() * 2 >= r,
                    _ => false,
                };
                twinned.insert(ci, ok_pairs(st, lt) && ok_pairs(sc, lc));
                if lt.is_none() && lc.is_none() {
                    continue;
                }
                let mut per_q = BTreeMap::new();
                let mut failed = false;
                for (q, tol) in tolerances {
                    let arm_div = |s: Option<&Arm>, l: Option<&Arm>| {
                        let l = l?;
                        Some(s.and_then(|s| {
                            let (ms, ml) = paired_mean(s, l, q, &both(s, l))?;
                            diverge(ms, ml, ms, *tol)
                        }))
                    };
                    let mut d = Divergence {
                        treatment: arm_div(st, lt),
                        control: arm_div(sc, lc),
                        effect: None,
                    };
                    if effect_qs.contains(q) {
                        if let (Some(st), Some(sc), Some(lt), Some(lc)) = (st, sc, lt, lc) {
                            let idx: Vec<usize> = both(st, lt)
                                .into_iter()
                                .filter(|i| {
                                    sc.replicates.get(*i).is_some_and(Option::is_some)
                                        && lc.replicates.get(*i).is_some_and(Option::is_some)
                                })
                                .collect();
                            d.effect = Some(
                                paired_mean(st, sc, q, &idx)
                                    .zip(paired_mean(lt, lc, q, &idx))
                                    .and_then(|((t_s, c_s), (t_l, c_l))| {
                                        diverge(t_s - c_s, t_l - c_l, c_s, *tol)
                                    }),
                            );
                        } else {
                            d.effect = Some(None);
                        }
                    }
                    if !d.within(*tol) {
                        failed = true;
                    }
                    per_q.insert(q.clone(), d);
                }
                if failed && h.design.twin_required {
                    twin_failed.push(key(&cd.cell));
                }
                divergences.insert(ci, per_q);
            }
        }

        // ---- the slice verdict (HYP-21) ----
        let evaluated: Vec<&CellData> = data
            .cells()
            .iter()
            .filter(|c| c.treatment.is_some())
            .collect();
        let mut reasons = Vec::new();
        let no_control: Vec<String> = evaluated
            .iter()
            .filter(|c| c.control.is_none())
            .map(|c| key(&c.cell))
            .collect();
        if matches!(h.control, Control::Missing) || !no_control.is_empty() {
            reasons.push(reason("control_missing", no_control));
        }
        if evaluated.is_empty() {
            reasons.push(reason("no_evaluated_cell", Vec::new()));
        }
        if !twin_failed.is_empty() {
            reasons.push(reason("twin_failed", twin_failed));
        }
        let gate = !reasons.is_empty();
        let incomplete: Vec<String> = evaluated
            .iter()
            .filter(|c| {
                let t = c.treatment.as_ref().map_or(0, Arm::completed);
                let k = c
                    .control
                    .and_then(|k| data.controls().get(k))
                    .map_or(0, |k| k.arm.completed());
                t < r || k < r
            })
            .map(|c| key(&c.cell))
            .collect();
        if !incomplete.is_empty() {
            reasons.push(reason("incomplete_cell", incomplete));
        }
        let missing: Vec<String> = data
            .cells()
            .iter()
            .filter(|c| c.treatment.is_none())
            .map(|c| key(&c.cell))
            .collect();
        if grid && !missing.is_empty() {
            reasons.push(reason("grid_cell_missing", missing));
        }
        if eval.outcome == Outcome::Undefined {
            let undefined: BTreeSet<String> = eval
                .values
                .iter()
                .filter(|(_, v)| matches!(v, Observed::Num(None) | Observed::Bool(None)))
                .map(|((e, _), _)| e.clone())
                .collect();
            reasons.push(reason("undefined_value", undefined.into_iter().collect()));
        }
        sort_reasons(&mut reasons);
        let v = if gate {
            V::Inconclusive
        } else {
            match eval.outcome {
                Outcome::Refuted => V::Fail,
                Outcome::NotRefuted => V::Pass,
                Outcome::Undefined => V::Inconclusive,
            }
        };
        let mut labels = BTreeSet::new();
        if has_sim && has_bundles {
            if !has_live {
                labels.insert("sim-only");
            } else if eval
                .decision_cells
                .iter()
                .any(|c| !twinned.get(c).copied().unwrap_or(false))
            {
                labels.insert("partially-twinned");
            }
        }
        let json = slice_json(
            h,
            &sk,
            sl,
            v,
            &reasons,
            &labels,
            &data,
            &eval,
            &divergences,
            &twinned,
            twin,
            seed,
        );
        slices.push((
            SliceVerdict {
                key: sk,
                params: sl.clone(),
                verdict: v,
                reasons,
                labels,
                has_bundles,
                data,
                eval,
            },
            json,
        ));
    }

    // ---- the file level (HYP-24) ----
    let conclusive = |s: &SliceVerdict| s.verdict != V::Inconclusive;
    let mut providers: BTreeMap<String, &'static str> = BTreeMap::new();
    let mut reported: BTreeSet<String> = BTreeSet::new();
    if has_provider && let Some(vals) = h.params.get("provider").and_then(|p| values_of(&p.domain))
    {
        for pv in vals {
            let mine: Vec<&SliceVerdict> = slices
                .iter()
                .map(|(s, _)| s)
                .filter(|s| s.params.get("provider") == Some(&pv))
                .collect();
            let status = if !mine.iter().any(|s| s.has_bundles) {
                "not_run"
            } else if mine.iter().all(|s| conclusive(s)) {
                reported.insert(pv.text());
                if mine.iter().any(|s| s.verdict == V::Fail) {
                    "fail"
                } else {
                    "pass"
                }
            } else {
                "inconclusive"
            };
            providers.insert(pv.text(), status);
        }
    }
    let counted: Vec<&SliceVerdict> = slices
        .iter()
        .map(|(s, _)| s)
        .filter(|s| {
            !has_provider
                || s.params
                    .get("provider")
                    .is_some_and(|p| reported.contains(&p.text()))
        })
        .collect();
    let replicates = counted.iter().filter_map(|s| s.data.min_replicates()).min();
    let counters = Counters {
        replicates: replicates.map(bootstrap::as_f64),
        providers: has_provider.then(|| bootstrap::as_f64(reported.len())),
    };
    let mut reasons = Vec::new();
    if let Some(g) = h.guard()
        && eval::truth(g, &counters) != Some(false)
    {
        reasons.push(reason("guard", vec![g.to_string()]));
    }
    let inconclusive_slices: Vec<String> = slices
        .iter()
        .map(|(s, _)| s)
        .filter(|s| !conclusive(s) && (s.has_bundles || !has_provider))
        .map(|s| s.key.clone())
        .collect();
    if !inconclusive_slices.is_empty() {
        reasons.push(reason("slice_inconclusive", inconclusive_slices));
    }
    if has_provider {
        let min = h.design.min_providers_for_verdict.unwrap_or(1);
        if reported.len() < usize::try_from(min).unwrap_or(usize::MAX) {
            reasons.push(reason(
                "providers_below_minimum",
                vec![format!("{} of {min}", reported.len())],
            ));
        }
    }
    if !slices.iter().any(|(s, _)| conclusive(s)) {
        reasons.push(reason("no_conclusive_slice", Vec::new()));
    }
    let v = if !reasons.is_empty() {
        V::Inconclusive
    } else if slices.iter().any(|(s, _)| s.verdict == V::Fail) {
        V::Fail
    } else {
        V::Pass
    };

    // ---- labels (HYP-23) ----
    let mut labels: BTreeSet<&'static str> = BTreeSet::new();
    if h.status() == Status::Candidate
        || infos
            .iter()
            .any(|i| i.b.manifest.hypothesis.status == "candidate")
    {
        labels.insert("exploratory");
    }
    if mock > 0 {
        labels.insert("mock-gated");
    }
    for (s, _) in &slices {
        labels.extend(s.labels.iter().copied());
    }
    if providers.values().any(|p| *p == "not_run") {
        labels.insert("partial-providers");
    }
    if h.status() == Status::Frozen
        && h.design.backends.iter().any(|b| b == "real-api")
        && h.design.pins.is_none()
    {
        labels.insert("unpinned-inputs");
    }

    // ---- verdict.json (HYP-15, HYP-28) ----
    let pairs: Vec<(Digest, Digest)> = bundles
        .iter()
        .map(|b| (b.run_id, b.bundle_digest))
        .collect();
    let id = verdict_id(&hash, &pairs)?;
    let builds: BTreeSet<String> = infos
        .iter()
        .map(|i| i.b.manifest.build.build_hash.clone())
        .collect();
    let json = J::obj([
        ("verdict_id", J::str(id.to_hex())),
        ("verdict", J::str(v.as_str())),
        ("reasons", reasons_json(&reasons)),
        ("labels", labels_json(&labels)),
        (
            "hypothesis",
            J::obj([
                ("id", J::str(h.id.clone())),
                ("status", J::str(h.status().as_str())),
                ("hash", J::str(hash.to_hex())),
            ]),
        ),
        (
            "expected",
            J::obj([
                ("outcome", J::str(h.expected_outcome.clone())),
                ("note", h.expected_note.clone().map_or(J::Null, J::str)),
            ]),
        ),
        (
            "providers",
            if has_provider {
                J::obj(providers.iter().map(|(k, s)| (k.clone(), J::str(*s))))
            } else {
                J::Null
            },
        ),
        (
            "bundles",
            J::Arr(
                infos
                    .iter()
                    .map(|i| {
                        J::obj([
                            ("run_id", J::str(i.b.run_id.to_hex())),
                            ("bundle_digest", J::str(i.b.bundle_digest.to_hex())),
                            ("mode", J::str(i.mode.as_str())),
                        ])
                    })
                    .collect(),
            ),
        ),
        (
            "ignored_replicates",
            J::Arr(
                ignored
                    .iter()
                    .map(|(run, idx)| {
                        J::obj([
                            ("run_id", J::str(run.clone())),
                            (
                                "replicates",
                                J::Arr(idx.iter().map(|i| J::Int(*i)).collect()),
                            ),
                        ])
                    })
                    .collect(),
            ),
        ),
        (
            "twin_only_cells",
            J::Arr(twin_only.iter().map(J::str).collect()),
        ),
        ("engine_hash", J::str(engine_hash.to_hex())),
        ("build_hashes", J::Arr(builds.iter().map(J::str).collect())),
        ("replicates", replicates.map_or(J::Null, J::count)),
        (
            "slices",
            J::Arr(slices.iter().map(|(_, j)| j.clone()).collect()),
        ),
    ]);
    Ok(Verdict {
        verdict_id: id,
        verdict: v,
        reasons,
        labels,
        slices: slices.into_iter().map(|(s, _)| s).collect(),
        providers,
        bundles: pairs,
        json,
    })
}

/// One slice of `verdict.json`: its parameters, verdict, reasons and labels; per
/// cell (in HYP-14 order) the arm values with replicate counts, the effect of
/// every primary quantity with its interval (CON-18), whether it decided the
/// value and is twinned, and its twin divergences; the controls; and every
/// sub-expression and term the falsifier read (HYP-28).
#[allow(clippy::too_many_arguments)]
fn slice_json(
    h: &Hypothesis,
    sk: &str,
    params: &Cell,
    v: V,
    reasons: &[Reason],
    labels: &BTreeSet<&'static str>,
    data: &SliceData,
    eval: &SliceEval,
    divergences: &BTreeMap<usize, BTreeMap<String, Divergence>>,
    twinned: &BTreeMap<usize, bool>,
    twin: bool,
    seed: u64,
) -> J {
    let measures: Vec<String> = h.measures().cloned().collect();
    let cell_key = |i: Option<usize>| {
        i.and_then(|i| data.cells().get(i))
            .map_or(J::Null, |c| J::str(key(&c.cell)))
    };
    let ctrl_key = |i: Option<usize>| {
        i.and_then(|i| data.controls().get(i))
            .map_or(J::Null, |c| J::str(key(&c.config)))
    };
    // HYP-11: an incomplete replicate, or one whose own value is undefined,
    // makes the arm undefined and is listed, never skipped.
    let arm_json = |a: &Arm| {
        let indices = |f: &dyn Fn(&Option<Values>) -> bool| {
            J::Arr(
                a.replicates
                    .iter()
                    .enumerate()
                    .filter(|(_, x)| f(x))
                    .map(|(i, _)| J::count(i))
                    .collect(),
            )
        };
        J::obj([
            ("completed", J::count(a.completed())),
            ("incomplete", indices(&|x| x.is_none())),
            (
                "undefined",
                J::obj(measures.iter().map(|q| {
                    (
                        q.clone(),
                        indices(&|x| {
                            x.as_ref()
                                .is_some_and(|v| v.get(q).copied().flatten().is_none())
                        }),
                    )
                })),
            ),
            (
                "values",
                J::obj(measures.iter().map(|q| (q.clone(), J::num(a.mean(q))))),
            ),
        ])
    };
    let cells: Vec<J> = data
        .cells()
        .iter()
        .enumerate()
        .map(|(ci, cd)| {
            let control = cd.control.and_then(|k| data.controls().get(k));
            let effects = J::obj(h.primary.iter().map(|q| {
                let pair = cd
                    .treatment
                    .as_ref()
                    .and_then(|t| t.values(q))
                    .zip(control.and_then(|k| k.arm.values(q)));
                let point = cd
                    .treatment
                    .as_ref()
                    .and_then(|t| t.mean(q))
                    .zip(control.and_then(|k| k.arm.mean(q)))
                    .map(|(t, c)| t - c);
                let ci = pair.and_then(|(t, c)| {
                    let name =
                        bootstrap::stream_name(data.key(), &key(&cd.cell), q, Function::Effect);
                    let stats =
                        bootstrap::effect_stats(&t, &c, bootstrap::stream(seed, &name).ok()?)?;
                    bootstrap::bounds(&stats, eval::DEFAULT_CI)
                });
                (
                    q.clone(),
                    J::obj([
                        ("value", J::num(point)),
                        ("ci_low", J::num(ci.map(|x| x.0))),
                        ("ci_high", J::num(ci.map(|x| x.1))),
                    ]),
                )
            }));
            J::obj([
                ("key", J::str(key(&cd.cell))),
                ("params", cell_json(&cd.cell)),
                ("evaluated", J::Bool(cd.treatment.is_some())),
                ("treatment", cd.treatment.as_ref().map_or(J::Null, arm_json)),
                ("control", ctrl_key(cd.control)),
                ("effects", effects),
                ("decision", J::Bool(eval.decision_cells.contains(&ci))),
                (
                    "twinned",
                    if twin {
                        twinned.get(&ci).copied().map_or(J::Null, J::Bool)
                    } else {
                        J::Null
                    },
                ),
                (
                    "divergence",
                    divergences.get(&ci).map_or(J::Null, |d| {
                        J::obj(d.iter().map(|(q, x)| (q.clone(), x.json())))
                    }),
                ),
            ])
        })
        .collect();
    let controls: Vec<J> = data
        .controls()
        .iter()
        .map(|k| {
            J::obj([
                ("key", J::str(key(&k.config))),
                (
                    "kind",
                    J::str(if k.kind == ControlKind::Workload {
                        "workload"
                    } else {
                        "config"
                    }),
                ),
                ("arm", arm_json(&k.arm)),
            ])
        })
        .collect();
    let values: Vec<J> = eval
        .values
        .iter()
        .map(|((e, at), o)| {
            J::obj([
                ("expr", J::str(e.clone())),
                ("at", cell_key(*at)),
                (
                    "value",
                    match o {
                        Observed::Num(x) => J::num(*x),
                        Observed::Bool(b) => b.map_or(J::Null, J::Bool),
                    },
                ),
            ])
        })
        .collect();
    let readings: Vec<J> = eval
        .readings
        .iter()
        .map(|((t, at, _), x)| {
            J::obj([
                ("term", J::str(t.clone())),
                ("at", cell_key(*at)),
                ("cell", cell_key(x.cell)),
                ("control", ctrl_key(x.control)),
                ("value", J::num(x.value)),
                ("completed", x.completed.map_or(J::Null, J::count)),
                ("ci_low", J::num(x.interval.map(|i| i.0))),
                ("ci_high", J::num(x.interval.map(|i| i.1))),
            ])
        })
        .collect();
    J::obj([
        ("key", J::str(sk)),
        ("params", cell_json(params)),
        ("verdict", J::str(v.as_str())),
        ("reasons", reasons_json(reasons)),
        ("labels", labels_json(labels)),
        (
            "outcome",
            J::str(match eval.outcome {
                Outcome::Refuted => "refuted",
                Outcome::NotRefuted => "not_refuted",
                Outcome::Undefined => "undefined",
            }),
        ),
        ("falsifier", eval.value.map_or(J::Null, J::Bool)),
        (
            "replicates",
            data.min_replicates().map_or(J::Null, J::count),
        ),
        ("cells", J::Arr(cells)),
        ("controls", J::Arr(controls)),
        (
            "decision_cells",
            J::Arr(
                eval.decision_cells
                    .iter()
                    .map(|i| cell_key(Some(*i)))
                    .collect(),
            ),
        ),
        ("values", J::Arr(values)),
        ("readings", J::Arr(readings)),
    ])
}

/// Write `runs_dir/verdicts/<verdict_id>/verdict.json`; an existing verdict
/// directory is never overwritten (HYP-20).
pub fn write(runs_dir: &Path, v: &Verdict) -> Result<PathBuf, Refusal> {
    let parent = runs_dir.join("verdicts");
    std::fs::create_dir_all(&parent).map_err(|e| Refusal(format!("{}: {e}", parent.display())))?;
    let dir = parent.join(v.verdict_id.to_hex());
    if let Err(e) = std::fs::create_dir(&dir) {
        return refuse(if e.kind() == std::io::ErrorKind::AlreadyExists {
            format!(
                "{} exists; a verdict is never overwritten (HYP-20)",
                dir.display()
            )
        } else {
            format!("{}: {e}", dir.display())
        });
    }
    let path = dir.join("verdict.json");
    std::fs::write(&path, v.text()).map_err(|e| Refusal(format!("{}: {e}", path.display())))?;
    Ok(path)
}
