//! The one path from bundles to a verdict (HYP-20..24, HYP-28; CON-17), in
//! phases: check each bundle and the set (HYP-20, HYP-21), build the arms from
//! the sessions, check the hashes that must agree, assemble each slice's cells
//! and controls, apply the twin rule (HYP-22), judge each slice (HYP-21), decide
//! the file under the guard (HYP-24), label the result (HYP-23), and render
//! `verdict.json` (HYP-15, HYP-28) from the typed result.

use std::collections::{BTreeMap, BTreeSet};
use std::path::{Path, PathBuf};

use acn_trace::bundle::MOCK_BACKEND;
use acn_trace::identity::{Digest, Mode, Preimage};

use crate::Status;
use crate::bootstrap;
use crate::eval::{self, CounterKind, Source, Term};
use crate::file::{Control, Domain, Hypothesis, Tolerance, as_int};
use crate::json::J;
use crate::predicate::{Arg, AtRange, Builtin, Expr, Lit};
use crate::quantities::{self, Replicate, Session};
use crate::read::BundleData;
use crate::slice::{
    Arm, Cell, CellData, ControlData, ControlKind, Evaluation, Observed, Outcome, SliceData,
    SliceEval, Value, Values, key,
};

/// The `format` of `verdict.json`; a change to its layout takes a new version.
pub const FORMAT: &str = "acn-bench/verdict/v1";

/// Why no verdict was written.
#[derive(Debug, Clone, PartialEq, Eq, thiserror::Error)]
pub enum VerdictError {
    /// HYP-20, HYP-21: the set cannot be judged.
    #[error("refused: {0}")]
    Refused(String),
    /// A verdict for this set exists and is never overwritten (HYP-20).
    #[error("{} exists; a verdict is never overwritten (HYP-20)", .0.display())]
    Exists(PathBuf),
    #[error("{}: {message}", path.display())]
    Io { path: PathBuf, message: String },
    /// An invariant of the engine failed: a bug, never data.
    #[error("internal: {0}")]
    Internal(String),
    /// The hypothesis file changed after it was loaded (HYP-4).
    #[error(transparent)]
    HypothesisChanged(#[from] crate::HypothesisChanged),
}

type Result<T> = std::result::Result<T, VerdictError>;

fn refuse<T>(m: impl Into<String>) -> Result<T> {
    Err(VerdictError::Refused(m.into()))
}

fn internal(e: impl std::fmt::Display) -> VerdictError {
    VerdictError::Internal(e.to_string())
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

/// The causes of HYP-21, in the order `reasons[]` lists them.
#[derive(Debug, Clone, Copy, PartialEq, Eq, PartialOrd, Ord)]
pub enum ReasonId {
    ControlMissing,
    NoEvaluatedCell,
    TwinFailed,
    IncompleteCell,
    GridCellMissing,
    UndefinedValue,
    Guard,
    SliceInconclusive,
    ProvidersBelowMinimum,
    NoConclusiveSlice,
}

impl ReasonId {
    #[must_use]
    pub fn as_str(self) -> &'static str {
        match self {
            Self::ControlMissing => "control_missing",
            Self::NoEvaluatedCell => "no_evaluated_cell",
            Self::TwinFailed => "twin_failed",
            Self::IncompleteCell => "incomplete_cell",
            Self::GridCellMissing => "grid_cell_missing",
            Self::UndefinedValue => "undefined_value",
            Self::Guard => "guard",
            Self::SliceInconclusive => "slice_inconclusive",
            Self::ProvidersBelowMinimum => "providers_below_minimum",
            Self::NoConclusiveSlice => "no_conclusive_slice",
        }
    }
}

/// One cause, with the cells or expressions it refers to.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct Reason {
    pub id: ReasonId,
    pub refers: Vec<String>,
}

/// The labels of HYP-23.
#[derive(Debug, Clone, Copy, PartialEq, Eq, PartialOrd, Ord)]
pub enum Label {
    Exploratory,
    MockGated,
    SimOnly,
    PartiallyTwinned,
    PartialProviders,
    UnpinnedInputs,
}

impl Label {
    #[must_use]
    pub fn as_str(self) -> &'static str {
        match self {
            Self::Exploratory => "exploratory",
            Self::MockGated => "mock-gated",
            Self::SimOnly => "sim-only",
            Self::PartiallyTwinned => "partially-twinned",
            Self::PartialProviders => "partial-providers",
            Self::UnpinnedInputs => "unpinned-inputs",
        }
    }
}

/// A declared provider value's status (HYP-24).
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum ProviderStatus {
    Pass,
    Fail,
    Inconclusive,
    NotRun,
}

impl ProviderStatus {
    #[must_use]
    pub fn as_str(self) -> &'static str {
        match self {
            Self::Pass => "pass",
            Self::Fail => "fail",
            Self::Inconclusive => "inconclusive",
            Self::NotRun => "not_run",
        }
    }
}

/// An arm (`acn.role`, TRC-10).
#[derive(Debug, Clone, Copy, PartialEq, Eq, PartialOrd, Ord)]
pub enum Role {
    Control,
    Treatment,
}

impl Role {
    #[must_use]
    pub fn as_str(self) -> &'static str {
        match self {
            Self::Control => "control",
            Self::Treatment => "treatment",
        }
    }

    #[must_use]
    pub fn parse(s: &str) -> Option<Self> {
        match s {
            "control" => Some(Self::Control),
            "treatment" => Some(Self::Treatment),
            _ => None,
        }
    }
}

/// An arm of one mode: its slice, role and configuration key (a treatment's
/// cell, or a control's own configuration).
#[derive(Debug, Clone, PartialEq, Eq, PartialOrd, Ord)]
pub struct ArmKey {
    pub slice: String,
    pub role: Role,
    pub config: String,
}

/// One cell's sim↔live divergence for one quantity (HYP-22). Each part is
/// `None` when not measured (no live arm, or not read through an effect) and
/// `Some(None)` when measured and undefined.
#[derive(Debug, Clone, Copy, PartialEq)]
pub struct Divergence {
    pub treatment: Option<Option<f64>>,
    pub control: Option<Option<f64>>,
    pub effect: Option<Option<f64>>,
}

impl Divergence {
    /// Within `tol` everywhere it was measured; undefined is not within.
    #[must_use]
    pub fn within(&self, tol: Tolerance) -> bool {
        [self.treatment, self.control, self.effect]
            .into_iter()
            .flatten()
            .all(|d| d.is_some_and(|d| d <= limit(tol)))
    }
}

/// The twin rule's findings for one slice (HYP-22).
#[derive(Debug, Clone, Default, PartialEq)]
pub struct TwinReport {
    /// Per cell (index into the slice's cells) with a live arm, per quantity.
    pub divergences: BTreeMap<usize, BTreeMap<String, Divergence>>,
    /// Per cell: both arms have live bundles with at least `replicates / 2`
    /// (rounded up) indices paired with sim.
    pub twinned: BTreeMap<usize, bool>,
    /// Cells outside a tolerance, or undefined, when `twin_required`.
    pub failed: Vec<String>,
}

/// One slice's verdict.
#[derive(Debug, Clone, PartialEq)]
pub struct SliceVerdict {
    pub key: String,
    pub params: Cell,
    pub verdict: V,
    pub reasons: Vec<Reason>,
    pub labels: BTreeSet<Label>,
    /// Whether the set holds any bundle of this slice (HYP-24).
    pub has_bundles: bool,
    /// The slice's data, as the falsifier read it.
    pub data: SliceData,
    pub eval: SliceEval,
    /// `None` when the set has no live twin to compare.
    pub twin: Option<TwinReport>,
    /// Per cell, per primary quantity: the effect and its 95% interval (CON-18).
    pub effects: BTreeMap<usize, BTreeMap<String, Effect>>,
}

/// A primary quantity's treatment-minus-control effect in one cell (CON-18).
#[derive(Debug, Clone, Copy, PartialEq)]
pub struct Effect {
    pub value: Option<f64>,
    /// The 95% interval, from the falsifier's own resamples (HYP-15).
    pub interval: Option<(f64, f64)>,
}

/// A verdict over a bundle set.
#[derive(Debug, Clone, PartialEq)]
pub struct Verdict {
    pub verdict_id: Digest,
    pub verdict: V,
    pub reasons: Vec<Reason>,
    pub labels: BTreeSet<Label>,
    pub slices: Vec<SliceVerdict>,
    /// Every declared provider value's status; `None` without a `provider`.
    pub providers: Option<BTreeMap<String, ProviderStatus>>,
    /// `(run_id, bundle_digest, mode)` of every bundle read, ascending.
    pub bundles: Vec<(Digest, Digest, Mode)>,
    /// Per `run_id`, the replicate indices ignored (HYP-21).
    pub ignored: BTreeMap<String, BTreeSet<i64>>,
    pub twin_only_cells: BTreeSet<String>,
    pub build_hashes: BTreeSet<String>,
    /// The guard's `replicates` counter (HYP-12).
    pub replicates: Option<usize>,
    pub engine_hash: Digest,
    json: J,
}

impl Verdict {
    /// `verdict.json`'s value.
    #[must_use]
    pub fn json(&self) -> &J {
        &self.json
    }

    /// The bytes of `verdict.json` (HYP-15).
    #[must_use]
    pub fn text(&self) -> String {
        self.json.render()
    }
}

/// HYP-9's run seed of a hypothesis (one definition, in `acn_trace::identity`).
pub fn hypothesis_seed(hash: &Digest) -> Result<u64> {
    acn_trace::identity::hypothesis_seed(hash).map_err(internal)
}

/// `verdict_id = blake3("acn-bench/verdict_id/v1\0" ‖ hypothesis_hash ‖ pairs)`,
/// each pair the raw `run_id` then `bundle_digest`, ascending (HYP-15).
pub fn verdict_id(hypothesis_hash: &Digest, bundles: &[(Digest, Digest)]) -> Result<Digest> {
    let mut p = Preimage::new("acn-bench/verdict_id/v1")
        .map_err(internal)?
        .digest(hypothesis_hash);
    for (r, d) in bundles {
        p = p.digest(r).digest(d);
    }
    Ok(p.finish())
}

/// Every value a finite parameter takes, in declaration order; load guarantees
/// that every non-pooled parameter, and every parameter of a grid, has one.
fn values_of(name: &str, d: &Domain) -> Result<Vec<Value>> {
    let v = match d {
        Domain::Bool => Some(vec![Value::Bool(false), Value::Bool(true)]),
        Domain::Enum(v) => Some(v.iter().map(|s| Value::Enum(s.clone())).collect()),
        Domain::Range {
            levels: Some(l), ..
        } => l.iter().map(|x| Value::float(*x)).collect(),
        Domain::IntRange {
            levels: Some(l), ..
        } => Some(l.iter().map(|x| Value::Int(*x)).collect()),
        _ => None,
    };
    v.ok_or_else(|| internal(format!("`{name}` has no finite set of values")))
}

/// Whether `v` is one of the grid's levels (or the domain has none).
fn on_grid(d: &Domain, v: &Value) -> bool {
    match (d, v) {
        (
            Domain::Range {
                levels: Some(l), ..
            },
            Value::Float(f),
        ) => l.contains(&f.get()),
        (
            Domain::IntRange {
                levels: Some(l), ..
            },
            Value::Int(i),
        ) => l.contains(i),
        _ => true,
    }
}

/// A `[control].config` literal as a cell value of its domain.
fn lit_value(d: &Domain, l: &Lit) -> Option<Value> {
    match (d, l) {
        (Domain::Bool, Lit::Bool(b)) => Some(Value::Bool(*b)),
        (Domain::Enum(_), Lit::Ident(s)) => Some(Value::Enum(s.clone())),
        (Domain::Range { .. }, Lit::Num(n)) => Value::float(*n),
        (Domain::IntRange { .. }, Lit::Num(n)) => as_int(*n).map(Value::Int),
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

fn limit(tol: Tolerance) -> f64 {
    match tol {
        Tolerance::Absolute(x) | Tolerance::Relative(x) => x,
    }
}

// ---- phase 1: each bundle (HYP-20) -----------------------------------------

/// What one bundle contributes, once checked.
struct Info<'a> {
    b: &'a BundleData,
    mode: Mode,
    cell: Cell,
    slice: Cell,
    role: Role,
    price_key: String,
}

/// What every bundle is checked against.
struct Context<'h> {
    h: &'h Hypothesis,
    hash: Digest,
    engine_hash: Digest,
    frozen_seed: u64,
    non_pooled: BTreeSet<String>,
    grid: bool,
}

fn check_bundle<'a>(cx: &Context<'_>, b: &'a BundleData) -> Result<Info<'a>> {
    let h = cx.h;
    let m = &b.manifest;
    let name = &m.run_id;
    if m.hypothesis.hash != cx.hash.to_hex() {
        return refuse(format!(
            "{name}: its hypothesis.hash is not the file's ({})",
            cx.hash.to_hex()
        ));
    }
    if m.engine_hash != cx.engine_hash.to_hex() {
        return refuse(format!(
            "{name}: engine_hash {} is not this tool's {} (CON-28, CON-31)",
            m.engine_hash,
            cx.engine_hash.to_hex()
        ));
    }
    let mode = Mode::parse(&m.mode).map_err(|e| VerdictError::Refused(format!("{name}: {e}")))?;
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
        if cx.grid && !on_grid(&param.domain, &v) {
            return refuse(format!(
                "{name}: `vary.{p} = {text}` is not one of the grid's levels (HYP-6, HYP-9)"
            ));
        }
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
    // HYP-9: `[design].backends` names the ways of providing inference the
    // hypothesis is about; a file that names none is about the mock only
    // (ADR-21). A real-provider bundle against a mock-only file is refused, so a
    // file cannot leave out `real-api` to escape its pins (HYP-23, HYP-26).
    let kind = if m.backend == MOCK_BACKEND {
        "mockllm"
    } else {
        "real-api"
    };
    let declared = &h.design.backends;
    let allowed = if declared.is_empty() {
        kind == "mockllm"
    } else {
        declared.iter().any(|d| d == kind)
    };
    if !allowed {
        return refuse(format!(
            "{name}: backend `{}` is {kind}, which [design].backends does not name (HYP-9; none named means mockllm only)",
            m.backend
        ));
    }
    let price_key = provider.unwrap_or_else(|| m.backend.clone());
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
    if h.status() == Status::Frozen && m.seed != cx.frozen_seed.to_string() {
        return refuse(format!(
            "{name}: run under seed {} but a frozen hypothesis runs under {} (HYP-9)",
            m.seed, cx.frozen_seed
        ));
    }
    // One arm per bundle (ADR-20), named exactly.
    let arms = m.params.get("arms").map(String::as_str).unwrap_or_default();
    let Some(role) = Role::parse(arms) else {
        return refuse(format!(
            "{name}: arms `{arms}` is not one arm, `control` or `treatment` (ADR-20)"
        ));
    };
    if let Some(bad) = b.sessions.iter().find(|s| s.role != arms) {
        return refuse(format!(
            "{name}: a session's role `{}` is not its arm `{arms}`",
            bad.role
        ));
    }
    Ok(Info {
        b,
        mode,
        slice: sub(&cell, &cx.non_pooled),
        cell,
        role,
        price_key,
    })
}

// ---- phase 2: the set (HYP-20, HYP-21) -------------------------------------

struct SetInfo {
    /// The mode the quantities come from.
    qmode: Mode,
    has_sim: bool,
    has_live: bool,
    /// Sim and live: the live bundles are the twin.
    twin: bool,
    mock: bool,
}

fn check_set(infos: &[Info<'_>]) -> Result<SetInfo> {
    let mut runs = BTreeSet::new();
    for i in infos {
        if !runs.insert(i.b.run_id) {
            return refuse(format!(
                "run {} is given twice (HYP-20)",
                i.b.run_id.to_hex()
            ));
        }
    }
    let mock = infos
        .iter()
        .filter(|i| i.b.manifest.backend == MOCK_BACKEND)
        .count();
    if mock != 0 && mock != infos.len() {
        return refuse("the set mixes mockllm and real backends (HYP-20)");
    }
    let methods: BTreeSet<&String> = infos.iter().flat_map(|i| &i.b.methods).collect();
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
    if has_netem && has_sim {
        return refuse(
            "netem bundles beside sim bundles are refused until the M4 rules are written (HYP-21)",
        );
    }
    if has_netem && has_live {
        return refuse(
            "live and netem bundles without sim are refused until the M4 rules are written (HYP-21)",
        );
    }
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
    for i in infos {
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
    let qmode = if has_sim {
        Mode::Sim
    } else if has_live {
        Mode::Live
    } else {
        Mode::Netem
    };
    Ok(SetInfo {
        qmode,
        has_sim,
        has_live,
        twin: has_sim && has_live,
        mock: mock > 0,
    })
}

// ---- the control's configuration (HYP-8) ----------------------------------

struct Controls<'h> {
    h: &'h Hypothesis,
    kind: ControlKind,
    /// A workload control's inherited parameters, with the non-pooled ones.
    keyed_by: BTreeSet<String>,
}

impl<'h> Controls<'h> {
    fn new(h: &'h Hypothesis, non_pooled: &BTreeSet<String>) -> Self {
        let (kind, mut keyed_by) = match &h.control {
            Control::Workload { inherits, .. } => (
                ControlKind::Workload,
                inherits.clone().map_or_else(
                    || {
                        h.params
                            .iter()
                            .filter(|(_, p)| p.domain.is_numeric())
                            .map(|(n, _)| n.clone())
                            .collect()
                    },
                    |v| v.into_iter().collect::<BTreeSet<_>>(),
                ),
            ),
            _ => (ControlKind::Config, BTreeSet::new()),
        };
        keyed_by.extend(non_pooled.iter().cloned());
        Self { h, kind, keyed_by }
    }

    /// The configuration of the control a treatment cell maps to; `None` when
    /// the file has no control.
    fn of_treatment(&self, c: &Cell) -> Option<Cell> {
        match &self.h.control {
            Control::Config(cfg) => {
                let mut e = c.clone();
                for (p, l) in cfg {
                    let d = &self.h.params.get(p)?.domain;
                    e.insert(p.clone(), lit_value(d, l)?);
                }
                Some(e)
            }
            Control::Workload { .. } => Some(sub(c, &self.keyed_by)),
            Control::Missing => None,
        }
    }

    /// A control bundle's own configuration: its cell, or what a workload
    /// control is keyed by.
    fn own(&self, c: &Cell) -> Cell {
        match self.kind {
            ControlKind::Workload => sub(c, &self.keyed_by),
            ControlKind::Config => c.clone(),
        }
    }
}

// ---- phase 3: arms from sessions (HYP-21) ----------------------------------

/// The arms of one mode.
#[derive(Default)]
struct Arms {
    arms: BTreeMap<ArmKey, Arm>,
    /// The scenario and workload hashes of the bundles behind each arm.
    hashes: BTreeMap<ArmKey, BTreeSet<(String, String)>>,
    configs: BTreeMap<ArmKey, Cell>,
}

type ByMode = BTreeMap<Mode, Arms>;
type Ignored = BTreeMap<String, BTreeSet<i64>>;

/// Build every arm. Indices at or beyond the design are ignored and listed; a
/// replicate completes only when each of its sessions ran at least one turn
/// (ADR-20); two bundles of one arm, mode and index are refused (HYP-20).
fn build_arms(
    h: &Hypothesis,
    infos: &[Info<'_>],
    controls: &Controls<'_>,
    r: usize,
) -> Result<(ByMode, Ignored)> {
    let measures: Vec<String> = h.measures().cloned().collect();
    let mut by_mode = ByMode::new();
    let mut ignored = Ignored::new();
    let mut covered: BTreeSet<(ArmKey, Mode, usize)> = BTreeSet::new();
    for i in infos {
        let b = i.b;
        let mut turns_of: BTreeMap<[u8; 8], Vec<usize>> = BTreeMap::new();
        for (k, t) in b.turns.iter().enumerate() {
            turns_of.entry(t.session_id).or_default().push(k);
        }
        let mut calls_of: BTreeMap<[u8; 8], Vec<usize>> = BTreeMap::new();
        for (k, c) in b.calls.iter().enumerate() {
            calls_of.entry(c.session_id).or_default().push(k);
        }
        let config = match i.role {
            Role::Control => controls.own(&i.cell),
            Role::Treatment => i.cell.clone(),
        };
        let akey = ArmKey {
            slice: key(&i.slice),
            role: i.role,
            config: key(&config),
        };
        let mut reps: BTreeMap<usize, Replicate> = BTreeMap::new();
        let mut unfinished: BTreeSet<usize> = BTreeSet::new();
        for s in &b.sessions {
            let Some(idx) = usize::try_from(s.replicate).ok().filter(|x| *x < r) else {
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
            let turns = turns_of.get(&s.session_id);
            if turns.is_none_or(Vec::is_empty) {
                unfinished.insert(idx);
            }
            for k in turns.into_iter().flatten() {
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
            if !covered.insert((akey.clone(), i.mode, idx)) {
                return refuse(format!(
                    "two bundles cover slice `{}`, {} `{}`, mode {}, replicate {idx} (HYP-20)",
                    akey.slice,
                    akey.role.as_str(),
                    akey.config,
                    i.mode.as_str()
                ));
            }
            if unfinished.contains(&idx) {
                continue;
            }
            let values: Values = measures
                .iter()
                .map(|q| (q.clone(), quantities::value(q, &rep)))
                .collect();
            if let Some(slot) = arm.replicates.get_mut(idx) {
                *slot = Some(values);
            }
        }
        arms.hashes.entry(akey.clone()).or_default().insert((
            b.manifest.scenario_hash.clone(),
            b.manifest.workload_hash.clone(),
        ));
        arms.configs.insert(akey, config);
    }
    Ok((by_mode, ignored))
}

// ---- phase 4: hashes that must agree (HYP-20) ------------------------------

fn only(hs: &BTreeSet<(String, String)>) -> Option<&(String, String)> {
    (hs.len() == 1).then(|| hs.iter().next()).flatten()
}

/// One arm's bundles share their scenario and workload; a treatment shares
/// both with its control (only the scenario for a workload control); a sim arm
/// shares both with its live twin.
fn check_hashes(by_mode: &ByMode, controls: &Controls<'_>, twin: bool) -> Result<()> {
    for (mode, arms) in by_mode {
        for (k, hs) in &arms.hashes {
            if hs.len() > 1 {
                return refuse(format!(
                    "the {} bundles of slice `{}`, {} `{}` differ in scenario or workload (HYP-20)",
                    mode.as_str(),
                    k.slice,
                    k.role.as_str(),
                    k.config
                ));
            }
        }
        for (k, hs) in &arms.hashes {
            if k.role != Role::Treatment {
                continue;
            }
            let Some(cc) = arms.configs.get(k).and_then(|c| controls.of_treatment(c)) else {
                continue;
            };
            let ck = ArmKey {
                slice: k.slice.clone(),
                role: Role::Control,
                config: key(&cc),
            };
            if let (Some(t), Some(c)) = (only(hs), arms.hashes.get(&ck).and_then(only)) {
                let workload_ok = controls.kind == ControlKind::Workload || t.1 == c.1;
                if t.0 != c.0 || !workload_ok {
                    return refuse(format!(
                        "cell `{}` and its control differ in scenario_hash or workload_hash (HYP-20)",
                        k.config
                    ));
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
                    k.config
                ));
            }
        }
    }
    Ok(())
}

// ---- phase 5: the twin rule (HYP-22) ----------------------------------------

/// The quantities a predicate reads through `effect`, `rel_effect`, `ci_low` or
/// `ci_high`.
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

/// The indices both arms completed.
fn both(a: &Arm, b: &Arm) -> Vec<usize> {
    (0..a.replicates.len().min(b.replicates.len()))
        .filter(|i| a.replicates[*i].is_some() && b.replicates[*i].is_some())
        .collect()
}

/// Each arm's mean of `q` over `idx`; undefined with no index or an undefined value.
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

/// `|live − sim|`, relative to `|base|` for a relative tolerance (a zero base
/// is undefined).
fn diverge(sim: f64, live: f64, base: f64, tol: Tolerance) -> Option<f64> {
    match tol {
        Tolerance::Absolute(_) => Some((live - sim).abs()),
        Tolerance::Relative(_) => (base != 0.0).then(|| (live - sim).abs() / base.abs()),
    }
}

fn twin_rule(
    h: &Hypothesis,
    data: &SliceData,
    sk: &str,
    live: &Arms,
    effect_qs: &BTreeSet<String>,
    r: usize,
) -> TwinReport {
    let mut out = TwinReport::default();
    let live_arm = |role: Role, config: String| {
        live.arms.get(&ArmKey {
            slice: sk.to_owned(),
            role,
            config,
        })
    };
    for (ci, cd) in data.cells().iter().enumerate() {
        let ctrl = cd.control.and_then(|k| data.controls().get(k));
        let lt = live_arm(Role::Treatment, key(&cd.cell));
        let lc = ctrl.and_then(|k| live_arm(Role::Control, key(&k.config)));
        let st = cd.treatment.as_ref();
        let sc = ctrl.map(|k| &k.arm);
        let paired = |s: Option<&Arm>, l: Option<&Arm>| match (s, l) {
            (Some(s), Some(l)) => both(s, l).len() * 2 >= r,
            _ => false,
        };
        out.twinned.insert(ci, paired(st, lt) && paired(sc, lc));
        if lt.is_none() && lc.is_none() {
            continue;
        }
        let mut per_q = BTreeMap::new();
        let mut failed = false;
        for (q, tol) in &h.design.sim_live_tolerance {
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
                d.effect = Some(match (st, sc, lt, lc) {
                    (Some(st), Some(sc), Some(lt), Some(lc)) => {
                        let idx: Vec<usize> = both(st, lt)
                            .into_iter()
                            .filter(|i| both(sc, lc).contains(i))
                            .collect();
                        paired_mean(st, sc, q, &idx)
                            .zip(paired_mean(lt, lc, q, &idx))
                            .and_then(|((t_s, c_s), (t_l, c_l))| {
                                diverge(t_s - c_s, t_l - c_l, c_s, *tol)
                            })
                    }
                    _ => None,
                });
            }
            if !d.within(*tol) {
                failed = true;
            }
            per_q.insert(q.clone(), d);
        }
        if failed && h.design.twin_required {
            out.failed.push(key(&cd.cell));
        }
        out.divergences.insert(ci, per_q);
    }
    out
}

// ---- phase 6: one slice (HYP-21) ---------------------------------------------

fn slice_reasons(
    h: &Hypothesis,
    data: &SliceData,
    eval: &SliceEval,
    twin: Option<&TwinReport>,
    grid: bool,
    r: usize,
) -> (Vec<Reason>, V) {
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
        reasons.push(Reason {
            id: ReasonId::ControlMissing,
            refers: no_control,
        });
    }
    if evaluated.is_empty() {
        reasons.push(Reason {
            id: ReasonId::NoEvaluatedCell,
            refers: Vec::new(),
        });
    }
    if let Some(t) = twin
        && !t.failed.is_empty()
    {
        reasons.push(Reason {
            id: ReasonId::TwinFailed,
            refers: t.failed.clone(),
        });
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
        reasons.push(Reason {
            id: ReasonId::IncompleteCell,
            refers: incomplete,
        });
    }
    let missing: Vec<String> = data
        .cells()
        .iter()
        .filter(|c| c.treatment.is_none())
        .map(|c| key(&c.cell))
        .collect();
    if grid && !missing.is_empty() {
        reasons.push(Reason {
            id: ReasonId::GridCellMissing,
            refers: missing,
        });
    }
    if eval.outcome == Outcome::Undefined {
        let undefined: BTreeSet<String> = eval
            .values
            .iter()
            .filter(|(_, v)| matches!(v, Observed::Num(None) | Observed::Bool(None)))
            .map(|((e, _), _)| e.clone())
            .collect();
        reasons.push(Reason {
            id: ReasonId::UndefinedValue,
            refers: undefined.into_iter().collect(),
        });
    }
    reasons.sort_by_key(|x| x.id);
    let v = if gate {
        V::Inconclusive
    } else {
        match eval.outcome {
            Outcome::Refuted => V::Fail,
            Outcome::NotRefuted => V::Pass,
            Outcome::Undefined => V::Inconclusive,
        }
    };
    (reasons, v)
}

// ---- phase 7: the file (HYP-24) -------------------------------------------------

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

struct FileLevel {
    verdict: V,
    reasons: Vec<Reason>,
    providers: Option<BTreeMap<String, ProviderStatus>>,
    replicates: Option<usize>,
}

fn file_level(h: &Hypothesis, slices: &[SliceVerdict]) -> Result<FileLevel> {
    let conclusive = |s: &SliceVerdict| s.verdict != V::Inconclusive;
    let provider_of = |s: &SliceVerdict| s.params.get("provider").map(Value::text);
    let mut providers = None;
    let mut reported: BTreeSet<String> = BTreeSet::new();
    let mut run: BTreeSet<String> = BTreeSet::new();
    if let Some(p) = h.params.get("provider") {
        let mut status = BTreeMap::new();
        for pv in values_of("provider", &p.domain)? {
            let pv = pv.text();
            let mine: Vec<&SliceVerdict> = slices
                .iter()
                .filter(|s| provider_of(s).as_deref() == Some(pv.as_str()))
                .collect();
            let st = if !mine.iter().any(|s| s.has_bundles) {
                ProviderStatus::NotRun
            } else {
                run.insert(pv.clone());
                if mine.iter().all(|s| conclusive(s)) {
                    reported.insert(pv.clone());
                    if mine.iter().any(|s| s.verdict == V::Fail) {
                        ProviderStatus::Fail
                    } else {
                        ProviderStatus::Pass
                    }
                } else {
                    ProviderStatus::Inconclusive
                }
            };
            status.insert(pv, st);
        }
        providers = Some(status);
    }
    let has_provider = providers.is_some();
    let replicates = slices
        .iter()
        .filter(|s| !has_provider || provider_of(s).is_some_and(|p| reported.contains(&p)))
        .filter_map(|s| s.data.min_replicates())
        .min();
    let counters = Counters {
        replicates: replicates.map(bootstrap::as_f64),
        providers: has_provider.then(|| bootstrap::as_f64(reported.len())),
    };
    let mut reasons = Vec::new();
    if let Some(g) = h.guard()
        && eval::truth(g, &counters) != Some(false)
    {
        reasons.push(Reason {
            id: ReasonId::Guard,
            refers: vec![g.to_string()],
        });
    }
    // A provider that was run counts every one of its slices: one cannot be run,
    // found wanting in a slice, and that slice left out (HYP-24, ADR-20).
    let inconclusive: Vec<String> = slices
        .iter()
        .filter(|s| {
            !conclusive(s)
                && (!has_provider
                    || s.has_bundles
                    || provider_of(s).is_some_and(|p| run.contains(&p)))
        })
        .map(|s| s.key.clone())
        .collect();
    if !inconclusive.is_empty() {
        reasons.push(Reason {
            id: ReasonId::SliceInconclusive,
            refers: inconclusive,
        });
    }
    if has_provider {
        // Load checks it lies in 1..=the number of providers.
        let min = h.design.min_providers_for_verdict.map_or(1, |m| m as usize);
        if reported.len() < min {
            reasons.push(Reason {
                id: ReasonId::ProvidersBelowMinimum,
                refers: vec![format!("{} of {min}", reported.len())],
            });
        }
    }
    if !slices.iter().any(conclusive) {
        reasons.push(Reason {
            id: ReasonId::NoConclusiveSlice,
            refers: Vec::new(),
        });
    }
    let verdict = if !reasons.is_empty() {
        V::Inconclusive
    } else if slices.iter().any(|s| s.verdict == V::Fail) {
        V::Fail
    } else {
        V::Pass
    };
    Ok(FileLevel {
        verdict,
        reasons,
        providers,
        replicates,
    })
}

// ---- the whole --------------------------------------------------------------

/// Judge `bundles` against `h` with the tool's own `engine_hash` (CON-28, CON-31).
pub fn verdict(
    h: &Hypothesis,
    mut bundles: Vec<BundleData>,
    engine_hash: Digest,
) -> Result<Verdict> {
    if bundles.is_empty() {
        return refuse("no bundles");
    }
    // HYP-15: ascending (run_id, bundle_digest), whatever the argument order.
    bundles.sort_by(|a, b| (a.run_id.0, a.bundle_digest.0).cmp(&(b.run_id.0, b.bundle_digest.0)));
    let r = usize::try_from(h.design.replicates).map_err(internal)?;
    let hash = h.hash();
    let non_pooled: BTreeSet<String> = h
        .params
        .iter()
        .filter(|(_, p)| !p.pooled)
        .map(|(n, _)| n.clone())
        .collect();
    let grid = h.design.search == "grid";
    let cx = Context {
        h,
        hash,
        engine_hash,
        frozen_seed: hypothesis_seed(&hash)?,
        non_pooled: non_pooled.clone(),
        grid,
    };
    let infos = bundles
        .iter()
        .map(|b| check_bundle(&cx, b))
        .collect::<Result<Vec<_>>>()?;
    let set = check_set(&infos)?;
    let controls = Controls::new(h, &non_pooled);
    let (by_mode, ignored) = build_arms(h, &infos, &controls, r)?;
    check_hashes(&by_mode, &controls, set.twin)?;

    // ---- slices (HYP-24) ----
    let param_values = |pooled: bool| -> Result<Vec<(String, Vec<Value>)>> {
        h.params
            .iter()
            .filter(|(n, _)| non_pooled.contains(*n) != pooled)
            .map(|(n, p)| Ok((n.clone(), values_of(n, &p.domain)?)))
            .collect()
    };
    let mut declared = product(&param_values(false)?);
    declared.sort_by_key(key);
    let one_slice = declared.len() == 1;
    let pooled_values = if grid {
        param_values(true)?
    } else {
        Vec::new()
    };
    let seed = bootstrap::verdict_seed(&hash).map_err(internal)?;
    let mut effect_qs = BTreeSet::new();
    effect_quantities(h.predicate(), &mut effect_qs);
    let empty = Arms::default();
    let q_arms = by_mode.get(&set.qmode).unwrap_or(&empty);
    let live_arms = if set.twin {
        by_mode.get(&Mode::Live)
    } else {
        None
    };

    let mut slices = Vec::new();
    let mut twin_only_cells = BTreeSet::new();
    for sl in &declared {
        let sk = key(sl);
        let has_bundles = infos.iter().any(|i| key(&i.slice) == sk);
        let sk_ref = sk.as_str();
        let mine = |role: Role| {
            q_arms
                .configs
                .iter()
                .filter(move |(k, _)| k.slice == sk_ref && k.role == role)
        };
        // Cells: every grid cell, or the cells the bundles ran.
        let universe: Vec<Cell> = if grid {
            product(&pooled_values)
                .into_iter()
                .map(|mut c| {
                    c.extend(sl.clone());
                    c
                })
                .collect()
        } else {
            mine(Role::Treatment).map(|(_, c)| c.clone()).collect()
        };
        let mut cdata: Vec<ControlData> = Vec::new();
        let mut control_index: BTreeMap<String, usize> = BTreeMap::new();
        for (k, c) in mine(Role::Control) {
            control_index.insert(k.config.clone(), cdata.len());
            cdata.push(ControlData {
                kind: controls.kind,
                config: c.clone(),
                arm: q_arms.arms.get(k).cloned().unwrap_or_default(),
            });
        }
        let cells: Vec<CellData> = universe
            .iter()
            .map(|c| CellData {
                cell: c.clone(),
                treatment: q_arms
                    .arms
                    .get(&ArmKey {
                        slice: sk.clone(),
                        role: Role::Treatment,
                        config: key(c),
                    })
                    .cloned(),
                control: controls
                    .of_treatment(c)
                    .and_then(|cc| control_index.get(&key(&cc)).copied()),
            })
            .collect();
        if let Some(live) = live_arms {
            for k in live.arms.keys() {
                if k.slice == sk && k.role == Role::Treatment && !q_arms.arms.contains_key(k) {
                    twin_only_cells.insert(k.config.clone());
                }
            }
        }
        let slice_key = if one_slice { String::new() } else { sk.clone() };
        let data = SliceData::new(h, slice_key, cells, cdata).map_err(internal)?;
        let ev = Evaluation::new(h.predicate(), &data, seed).map_err(internal)?;
        let eval = ev.run(h.predicate());
        let twin = live_arms.map(|live| twin_rule(h, &data, &sk, live, &effect_qs, r));
        let (reasons, v) = slice_reasons(h, &data, &eval, twin.as_ref(), grid, r);
        let mut labels = BTreeSet::new();
        if set.has_sim && has_bundles {
            if !set.has_live {
                labels.insert(Label::SimOnly);
            } else if let Some(t) = &twin
                && eval
                    .decision_cells
                    .iter()
                    .any(|c| !t.twinned.get(c).copied().unwrap_or(false))
            {
                labels.insert(Label::PartiallyTwinned);
            }
        }
        // CON-18: every primary quantity's effect, with its interval from the
        // falsifier's own cached resamples.
        let mut effects = BTreeMap::new();
        for q in &h.primary {
            ev.check_effect_streams(q).map_err(internal)?;
        }
        for (ci, cd) in data.cells().iter().enumerate() {
            let control = cd.control.and_then(|k| data.controls().get(k));
            let per: BTreeMap<String, Effect> = h
                .primary
                .iter()
                .map(|q| {
                    let point = cd
                        .treatment
                        .as_ref()
                        .and_then(|t| t.mean(q))
                        .zip(control.and_then(|k| k.arm.mean(q)))
                        .map(|(t, c)| t - c);
                    (
                        q.clone(),
                        Effect {
                            value: point,
                            interval: ev.effect_interval(ci, q, eval::DEFAULT_CI),
                        },
                    )
                })
                .collect();
            effects.insert(ci, per);
        }
        drop(ev);
        slices.push(SliceVerdict {
            key: sk,
            params: sl.clone(),
            verdict: v,
            reasons,
            labels,
            has_bundles,
            data,
            eval,
            twin,
            effects,
        });
    }

    let file = file_level(h, &slices)?;

    // ---- labels (HYP-23) ----
    let mut labels = BTreeSet::new();
    if h.status() == Status::Candidate
        || infos
            .iter()
            .any(|i| i.b.manifest.hypothesis.status == "candidate")
    {
        labels.insert(Label::Exploratory);
    }
    if set.mock {
        labels.insert(Label::MockGated);
    }
    for s in &slices {
        labels.extend(s.labels.iter().copied());
    }
    if file
        .providers
        .as_ref()
        .is_some_and(|p| p.values().any(|s| *s == ProviderStatus::NotRun))
    {
        labels.insert(Label::PartialProviders);
    }
    if h.status() == Status::Frozen
        && h.design.backends.iter().any(|b| b == "real-api")
        && h.design.pins.is_none()
    {
        labels.insert(Label::UnpinnedInputs);
    }

    let pairs: Vec<(Digest, Digest)> = bundles
        .iter()
        .map(|b| (b.run_id, b.bundle_digest))
        .collect();
    let mut v = Verdict {
        verdict_id: verdict_id(&hash, &pairs)?,
        verdict: file.verdict,
        reasons: file.reasons,
        labels,
        slices,
        providers: file.providers,
        bundles: infos
            .iter()
            .map(|i| (i.b.run_id, i.b.bundle_digest, i.mode))
            .collect(),
        ignored,
        twin_only_cells,
        build_hashes: infos
            .iter()
            .map(|i| i.b.manifest.build.build_hash.clone())
            .collect(),
        replicates: file.replicates,
        engine_hash,
        json: J::Null,
    };
    v.json = render(h, &v, set.twin);
    Ok(v)
}

// ---- verdict.json (HYP-15, HYP-28) -------------------------------------------

fn reasons_json(r: &[Reason]) -> J {
    J::Arr(
        r.iter()
            .map(|r| {
                J::obj([
                    ("reason", J::str(r.id.as_str())),
                    ("refers", J::Arr(r.refers.iter().map(J::str).collect())),
                ])
            })
            .collect(),
    )
}

fn labels_json(l: &BTreeSet<Label>) -> J {
    let names: BTreeSet<&str> = l.iter().map(|x| x.as_str()).collect();
    J::Arr(names.into_iter().map(J::str).collect())
}

fn cell_json(c: &Cell) -> J {
    J::obj(c.iter().map(|(k, v)| (k.clone(), J::str(v.text()))))
}

fn divergence_json(d: &Divergence) -> J {
    let f = |x: Option<Option<f64>>| x.map_or(J::Null, J::num);
    J::obj([
        ("control", f(d.control)),
        ("effect", f(d.effect)),
        ("treatment", f(d.treatment)),
    ])
}

/// The value of `verdict.json`: rendered from the typed verdict alone.
fn render(h: &Hypothesis, v: &Verdict, twin: bool) -> J {
    J::obj([
        ("format", J::str(FORMAT)),
        ("verdict_id", J::str(v.verdict_id.to_hex())),
        ("verdict", J::str(v.verdict.as_str())),
        ("reasons", reasons_json(&v.reasons)),
        ("labels", labels_json(&v.labels)),
        (
            "hypothesis",
            J::obj([
                ("id", J::str(h.id.clone())),
                ("status", J::str(h.status().as_str())),
                ("hash", J::str(h.hash().to_hex())),
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
            v.providers.as_ref().map_or(J::Null, |p| {
                J::obj(p.iter().map(|(k, s)| (k.clone(), J::str(s.as_str()))))
            }),
        ),
        (
            "bundles",
            J::Arr(
                v.bundles
                    .iter()
                    .map(|(r, d, m)| {
                        J::obj([
                            ("run_id", J::str(r.to_hex())),
                            ("bundle_digest", J::str(d.to_hex())),
                            ("mode", J::str(m.as_str())),
                        ])
                    })
                    .collect(),
            ),
        ),
        (
            "ignored_replicates",
            J::Arr(
                v.ignored
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
            J::Arr(v.twin_only_cells.iter().map(J::str).collect()),
        ),
        ("engine_hash", J::str(v.engine_hash.to_hex())),
        (
            "build_hashes",
            J::Arr(v.build_hashes.iter().map(J::str).collect()),
        ),
        ("replicates", v.replicates.map_or(J::Null, J::count)),
        (
            "slices",
            J::Arr(v.slices.iter().map(|s| slice_json(h, s, twin)).collect()),
        ),
    ])
}

/// One slice: its parameters, verdict, reasons and labels; per cell (in HYP-14
/// order) the arm values with replicate counts and the replicates listed under
/// HYP-11, the effect of every primary quantity with its interval (CON-18),
/// whether it decided the value and is twinned, and its divergences; the
/// controls; and every sub-expression and term the falsifier read (HYP-28).
fn slice_json(h: &Hypothesis, s: &SliceVerdict, twin: bool) -> J {
    let data = &s.data;
    let eval = &s.eval;
    let measures: Vec<String> = h.measures().cloned().collect();
    let cell_key = |i: Option<usize>| {
        i.and_then(|i| data.cells().get(i))
            .map_or(J::Null, |c| J::str(key(&c.cell)))
    };
    let ctrl_key = |i: Option<usize>| {
        i.and_then(|i| data.controls().get(i))
            .map_or(J::Null, |c| J::str(key(&c.config)))
    };
    // HYP-11: an incomplete replicate, or one whose own value is undefined, makes
    // the arm undefined and is listed, never skipped.
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
            let effects = s.effects.get(&ci).map_or(J::Null, |m| {
                J::obj(m.iter().map(|(q, e)| {
                    (
                        q.clone(),
                        J::obj([
                            ("value", J::num(e.value)),
                            ("ci_low", J::num(e.interval.map(|x| x.0))),
                            ("ci_high", J::num(e.interval.map(|x| x.1))),
                        ]),
                    )
                }))
            });
            let (twinned, divergence) = match &s.twin {
                Some(t) if twin => (
                    t.twinned.get(&ci).copied().map_or(J::Null, J::Bool),
                    t.divergences.get(&ci).map_or(J::Null, |d| {
                        J::obj(d.iter().map(|(q, x)| (q.clone(), divergence_json(x))))
                    }),
                ),
                _ => (J::Null, J::Null),
            };
            J::obj([
                ("key", J::str(key(&cd.cell))),
                ("params", cell_json(&cd.cell)),
                ("evaluated", J::Bool(cd.treatment.is_some())),
                ("treatment", cd.treatment.as_ref().map_or(J::Null, arm_json)),
                ("control", ctrl_key(cd.control)),
                ("effects", effects),
                ("decision", J::Bool(eval.decision_cells.contains(&ci))),
                ("twinned", twinned),
                ("divergence", divergence),
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
                    J::str(match k.kind {
                        ControlKind::Workload => "workload",
                        ControlKind::Config => "config",
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
        ("key", J::str(s.key.clone())),
        ("params", cell_json(&s.params)),
        ("verdict", J::str(s.verdict.as_str())),
        ("reasons", reasons_json(&s.reasons)),
        ("labels", labels_json(&s.labels)),
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

// ---- writing (HYP-20, HYP-4) -------------------------------------------------

/// Judge, then write: the one call that turns a loaded hypothesis and its
/// bundles into `verdict.json`. HYP-4: the file is read again before the write,
/// and a file that changed since it was loaded aborts the write with
/// `hypothesis_changed`, so a verdict always belongs to the bytes it was judged
/// against.
pub fn judge_and_write(
    h: &Hypothesis,
    bundles: Vec<BundleData>,
    engine_hash: Digest,
    runs_dir: &Path,
) -> Result<(Verdict, PathBuf)> {
    let v = verdict(h, bundles, engine_hash)?;
    h.check_unchanged()?;
    let path = write(runs_dir, &v)?;
    Ok((v, path))
}

/// HYP-4: where verdicts may go. `runs_dir` is named `runs`, is a real
/// directory (never a symbolic link), and, when a workspace root (CON-28) holds
/// it, is that root's own `runs/`: so neither `hypotheses/runs` nor any other
/// directory named `runs` inside a workspace receives a verdict.
fn check_runs_dir(runs_dir: &Path) -> Result<()> {
    let refuse_it = |why: &str| {
        refuse(format!(
            "{} is not a `runs` directory verdicts may be written to: {why} (HYP-4)",
            runs_dir.display()
        ))
    };
    if runs_dir.file_name().and_then(|n| n.to_str()) != Some("runs") {
        return refuse_it("its name is not `runs`");
    }
    if std::fs::symlink_metadata(runs_dir).is_ok_and(|m| !m.is_dir()) {
        return refuse_it("it is not a directory");
    }
    let parent = match runs_dir.parent() {
        Some(p) if !p.as_os_str().is_empty() => p.to_path_buf(),
        _ => PathBuf::from("."),
    };
    let parent = std::fs::canonicalize(&parent).map_err(|e| VerdictError::Io {
        path: parent.clone(),
        message: e.to_string(),
    })?;
    match acn_trace::env::find_root(&parent).map_err(internal)? {
        Some(root) if root != parent => refuse_it(&format!(
            "it lies inside the workspace {} but is not its own runs/",
            root.display()
        )),
        _ => Ok(()),
    }
}

/// Write `runs_dir/verdicts/<verdict_id>/verdict.json`.
/// - HYP-4: `runs_dir` is checked as `check_runs_dir` says, and neither
///   `verdicts/` nor the verdict's directory may be a symbolic link.
/// - HYP-20: an existing `verdict.json` is never overwritten.
/// - Atomic: the bytes go to a sibling temporary file that is then linked into
///   place (which fails if the target exists), so a crash leaves no partial
///   verdict and never blocks a later attempt.
pub fn write(runs_dir: &Path, v: &Verdict) -> Result<PathBuf> {
    check_runs_dir(runs_dir)?;
    let io = |path: &Path| {
        let path = path.to_path_buf();
        move |e: std::io::Error| VerdictError::Io {
            path,
            message: e.to_string(),
        }
    };
    // Each level is created, then checked not to be a symbolic link, before
    // anything is created beneath it.
    let verdicts = runs_dir.join("verdicts");
    let dir = verdicts.join(v.verdict_id.to_hex());
    for d in [runs_dir, verdicts.as_path(), dir.as_path()] {
        if let Err(e) = std::fs::create_dir(d)
            && e.kind() != std::io::ErrorKind::AlreadyExists
        {
            return Err(io(d)(e));
        }
        if !std::fs::symlink_metadata(d).map_err(io(d))?.is_dir() {
            return refuse(format!(
                "{} is not a directory, or is a symbolic link (HYP-4)",
                d.display()
            ));
        }
    }
    let path = dir.join("verdict.json");
    if path.exists() {
        return Err(VerdictError::Exists(dir));
    }
    let tmp = dir.join(".verdict.json.partial");
    let _ = std::fs::remove_file(&tmp);
    let result = (|| {
        use std::io::Write as _;
        let mut f = std::fs::OpenOptions::new()
            .write(true)
            .create_new(true)
            .open(&tmp)
            .map_err(io(&tmp))?;
        f.write_all(v.text().as_bytes()).map_err(io(&tmp))?;
        f.sync_all().map_err(io(&tmp))?;
        std::fs::hard_link(&tmp, &path).map_err(|e| {
            if e.kind() == std::io::ErrorKind::AlreadyExists {
                VerdictError::Exists(dir.clone())
            } else {
                io(&path)(e)
            }
        })
    })();
    let _ = std::fs::remove_file(&tmp);
    result.map(|()| path)
}
