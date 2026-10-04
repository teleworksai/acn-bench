//! A falsifier evaluated over one slice's data (HYP-11..14). The data are the
//! per-replicate values of each quantity, per arm and cell, which the verdict
//! engine reads from bundles; here they are evaluated by the one evaluator of
//! [`crate::eval`], so lint's probes and real verdicts share every rule.
//!
//! An evaluation visits every cell it ranges over, even where Kleene's rules
//! have already decided, and records every value it computes. That record is
//! what HYP-28 prints, what HYP-22's decision cells come from, and what tells a
//! clean `false` (which may pass) from a `false` reached past an undefined
//! operand (which may not, HYP-21).

use std::cell::RefCell;
use std::cmp::Ordering;
use std::collections::{BTreeMap, BTreeSet};
use std::rc::Rc;

use acn_trace::identity::float_text;

use crate::bootstrap::{self, Function};
use crate::eval::{self, CounterKind, Source, Term};
use crate::file::{Domain, Hypothesis};
use crate::predicate::{AtRange, Expr};

/// Slice data that cannot be evaluated: an engine or caller bug, never data.
#[derive(Debug, Clone, PartialEq, Eq, thiserror::Error)]
#[error("slice data: {0}")]
pub struct SliceError(pub String);

fn fail<T>(m: impl Into<String>) -> Result<T, SliceError> {
    Err(SliceError(m.into()))
}

/// A finite double with its text form of CON-27(c), fixed at construction.
#[derive(Debug, Clone, PartialEq)]
pub struct Finite {
    v: f64,
    text: String,
}

impl Finite {
    /// `None` for NaN and the infinities.
    #[must_use]
    pub fn new(v: f64) -> Option<Self> {
        let text = float_text(v).ok()?;
        // Negative zero is zero (CON-27(c)).
        Some(Self { v: v + 0.0, text })
    }

    #[must_use]
    pub fn get(&self) -> f64 {
        self.v
    }
}

/// A parameter's value in one cell (HYP-6), as the manifest's `vary.<name>` holds it.
#[derive(Debug, Clone, PartialEq)]
pub enum Value {
    Bool(bool),
    Enum(String),
    Int(i64),
    Float(Finite),
}

impl Value {
    /// A `range` value; `None` unless finite.
    #[must_use]
    pub fn float(v: f64) -> Option<Self> {
        Finite::new(v).map(Self::Float)
    }

    /// The value of a parameter with domain `domain` from its text in the
    /// manifest's `params` (CON-27(c)); `None` when the text is not a value of
    /// the domain.
    #[must_use]
    pub fn parse(domain: &Domain, text: &str) -> Option<Self> {
        let v = match domain {
            Domain::Bool => match text {
                "true" => Self::Bool(true),
                "false" => Self::Bool(false),
                _ => return None,
            },
            Domain::Enum(_) => Self::Enum(text.to_owned()),
            Domain::IntRange { .. } => Self::Int(text.parse().ok()?),
            Domain::Range { .. } => {
                let f = Finite::new(text.parse().ok()?)?;
                if f.text != text {
                    return None;
                }
                Self::Float(f)
            }
        };
        v.fits(domain).then_some(v)
    }

    /// Whether the value lies in `domain` (and is of its kind).
    #[must_use]
    pub fn fits(&self, domain: &Domain) -> bool {
        match (self, domain) {
            (Self::Bool(_), Domain::Bool) => true,
            (Self::Enum(s), Domain::Enum(vals)) => vals.contains(s),
            (Self::Int(i), Domain::IntRange { min, max, .. }) => min <= i && i <= max,
            (Self::Float(f), Domain::Range { min, max, .. }) => *min <= f.v && f.v <= *max,
            _ => false,
        }
    }

    /// The text form of CON-27(c).
    #[must_use]
    pub fn text(&self) -> String {
        match self {
            Self::Bool(b) => b.to_string(),
            Self::Enum(s) => s.clone(),
            Self::Int(i) => i.to_string(),
            Self::Float(f) => f.text.clone(),
        }
    }

    /// The value as a number, for an `at` bound.
    #[must_use]
    pub fn num(&self) -> Option<f64> {
        match self {
            #[allow(clippy::cast_precision_loss)] // int_range values are checked to be exact
            Self::Int(i) => Some(*i as f64),
            Self::Float(f) => Some(f.v),
            Self::Bool(_) | Self::Enum(_) => None,
        }
    }

    /// Whether a selector's value (as [`crate::eval::lit_text`] writes it) names
    /// this value: integers compare as integers, never through a double.
    #[must_use]
    pub fn matches(&self, text: &str) -> bool {
        match self {
            Self::Bool(b) => text == if *b { "true" } else { "false" },
            Self::Enum(s) => text == s,
            Self::Int(i) => text.parse::<i64>() == Ok(*i),
            Self::Float(f) => text.parse::<f64>() == Ok(f.v),
        }
    }
}

/// One cell: a value for every `[varies]` parameter (pooled or not).
pub type Cell = BTreeMap<String, Value>;

/// A cell's key (HYP-15): `name=value` pairs sorted by name, joined by `,`.
#[must_use]
pub fn key(cell: &Cell) -> String {
    cell.iter()
        .map(|(k, v)| format!("{k}={}", v.text()))
        .collect::<Vec<_>>()
        .join(",")
}

/// HYP-14's order of cells: by parameter name, then by value — numerically for
/// numbers, `false` before `true`, enum values in declaration order. Total:
/// values the rule does not order (which a validated slice never holds) fall
/// back to their text, and cells to their keys.
#[must_use]
pub fn cell_order(h: &Hypothesis, a: &Cell, b: &Cell) -> Ordering {
    for ((name, va), (nb, vb)) in a.iter().zip(b) {
        let o = name.cmp(nb).then_with(|| match (va, vb) {
            (Value::Bool(x), Value::Bool(y)) => x.cmp(y),
            (Value::Enum(x), Value::Enum(y)) => {
                let pos = |v: &String| match h.params.get(name).map(|p| &p.domain) {
                    Some(Domain::Enum(vals)) => vals.iter().position(|w| w == v),
                    _ => None,
                };
                match (pos(x), pos(y)) {
                    (Some(i), Some(j)) => i.cmp(&j),
                    _ => x.cmp(y),
                }
            }
            (Value::Int(x), Value::Int(y)) => x.cmp(y),
            (Value::Float(x), Value::Float(y)) => x.v.total_cmp(&y.v),
            _ => va.text().cmp(&vb.text()),
        });
        if o != Ordering::Equal {
            return o;
        }
    }
    key(a).cmp(&key(b))
}

/// One replicate's values: quantity → value, `None` when undefined (HYP-11).
pub type Values = BTreeMap<String, Option<f64>>;

/// One arm of one cell: the replicates with index `0..[design].replicates`,
/// each `None` unless it completed. [`SliceData::new`] checks the length.
#[derive(Debug, Clone, Default, PartialEq)]
pub struct Arm {
    pub replicates: Vec<Option<Values>>,
}

impl Arm {
    /// The number of completed replicates.
    #[must_use]
    pub fn completed(&self) -> usize {
        self.replicates.iter().filter(|r| r.is_some()).count()
    }

    /// Every replicate's value of `q`, in index order; `None` unless every one
    /// completed and each value is defined (HYP-11: an undefined replicate makes
    /// the arm undefined, and is never skipped).
    #[must_use]
    pub fn values(&self, q: &str) -> Option<Vec<f64>> {
        if self.replicates.is_empty() {
            return None;
        }
        self.replicates
            .iter()
            .map(|r| r.as_ref().and_then(|v| v.get(q).copied().flatten()))
            .collect()
    }

    /// The arm's value of `q`: the mean of its per-replicate values, summed in
    /// index order.
    #[must_use]
    pub fn mean(&self, q: &str) -> Option<f64> {
        let v = self.values(q)?;
        let mut sum = 0.0;
        for x in &v {
            sum += x;
        }
        Some(sum / bootstrap::as_f64(v.len())).filter(|m| m.is_finite())
    }
}

/// One treatment cell of the slice.
#[derive(Debug, Clone, PartialEq)]
pub struct CellData {
    pub cell: Cell,
    /// `None` for a grid cell with no runs (undefined, HYP-21).
    pub treatment: Option<Arm>,
    /// The index in the slice's controls of the control arm it maps to (HYP-8).
    pub control: Option<usize>,
}

/// The two forms of control of HYP-8.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum ControlKind {
    /// `[control].config`: keyed by its effective configuration.
    Config,
    /// `[control].workload`: keyed by the parameters it inherits.
    Workload,
}

/// One control arm, keyed by its configuration (HYP-8).
#[derive(Debug, Clone, PartialEq)]
pub struct ControlData {
    pub kind: ControlKind,
    /// Every parameter for a config control; the inherited ones for a workload
    /// control. Its key is the control's cell key in `noise_floor`'s sub-stream.
    pub config: Cell,
    pub arm: Arm,
}

/// Everything one slice's verdict reads, checked against the hypothesis.
#[derive(Debug, Clone, PartialEq)]
pub struct SliceData {
    key: String,
    cells: Vec<CellData>,
    controls: Vec<ControlData>,
}

impl SliceData {
    /// Check the data against `h` and put the cells in HYP-14 order: every cell
    /// assigns every `[varies]` parameter a value of its domain, cell and control
    /// keys are unique, every arm holds exactly `[design].replicates` entries
    /// (indices at or beyond it are ignored before this, HYP-21), and every
    /// control index exists.
    pub fn new(
        h: &Hypothesis,
        key: String,
        mut cells: Vec<CellData>,
        controls: Vec<ControlData>,
    ) -> Result<Self, SliceError> {
        let n = usize::try_from(h.design.replicates).unwrap_or(usize::MAX);
        let arm_ok = |a: &Arm, what: &str| {
            if a.replicates.len() == n {
                Ok(())
            } else {
                fail(format!(
                    "{what} holds {} replicates, not the design's {n}",
                    a.replicates.len()
                ))
            }
        };
        let check_values = |c: &Cell, all: bool, what: &str| -> Result<(), SliceError> {
            for (p, v) in c {
                match h.params.get(p) {
                    Some(param) if v.fits(&param.domain) => {}
                    Some(_) => {
                        return fail(format!("{what}: `{p} = {}` is not in its domain", v.text()));
                    }
                    None => return fail(format!("{what}: `{p}` is not a [varies] parameter")),
                }
            }
            if all && c.len() != h.params.len() {
                return fail(format!("{what} does not assign every [varies] parameter"));
            }
            Ok(())
        };
        let mut seen = BTreeSet::new();
        for (i, k) in controls.iter().enumerate() {
            let what = format!("control {}", key_of(&k.config));
            check_values(&k.config, k.kind == ControlKind::Config, &what)?;
            arm_ok(&k.arm, &what)?;
            if !seen.insert(key_of(&k.config)) {
                return fail(format!("{what} appears twice (index {i})"));
            }
        }
        let mut seen = BTreeSet::new();
        for c in &cells {
            let what = format!("cell {}", key_of(&c.cell));
            check_values(&c.cell, true, &what)?;
            if let Some(t) = &c.treatment {
                arm_ok(t, &what)?;
            }
            if c.control.is_some_and(|i| i >= controls.len()) {
                return fail(format!("{what} maps to a control that does not exist"));
            }
            if !seen.insert(key_of(&c.cell)) {
                return fail(format!("{what} appears twice"));
            }
        }
        cells.sort_by(|a, b| cell_order(h, &a.cell, &b.cell));
        Ok(Self {
            key,
            cells,
            controls,
        })
    }

    /// The slice key (HYP-15); empty for a file with one slice.
    #[must_use]
    pub fn key(&self) -> &str {
        &self.key
    }

    /// The cells, in HYP-14 order.
    #[must_use]
    pub fn cells(&self) -> &[CellData] {
        &self.cells
    }

    #[must_use]
    pub fn controls(&self) -> &[ControlData] {
        &self.controls
    }

    /// The counter `replicates` within the slice (HYP-12): the minimum over its
    /// evaluated cells and both arms of the completed replicates; a missing
    /// control counts as none. A grid cell with no runs is not evaluated and
    /// does not count.
    #[must_use]
    pub fn min_replicates(&self) -> Option<usize> {
        self.cells
            .iter()
            .filter_map(|c| {
                let t = c.treatment.as_ref()?.completed();
                let k = c
                    .control
                    .and_then(|i| self.controls.get(i))
                    .map_or(0, |k| k.arm.completed());
                Some(t.min(k))
            })
            .min()
    }
}

fn key_of(c: &Cell) -> String {
    key(c)
}

/// The value of one sub-expression in one context.
#[derive(Debug, Clone, Copy, PartialEq)]
pub enum Observed {
    Num(Option<f64>),
    Bool(Option<bool>),
}

/// One term read in one context: which arm it resolved to, its value, the
/// replicates behind it and, for an interval bound, the interval.
#[derive(Debug, Clone, PartialEq)]
pub struct Reading {
    /// The treatment cell (index into [`SliceData::cells`]) the term resolved to.
    pub cell: Option<usize>,
    /// The control (index into [`SliceData::controls`]) it read, if any.
    pub control: Option<usize>,
    pub value: Option<f64>,
    /// Completed replicates in the arm read.
    pub completed: Option<usize>,
    /// `(ci_low, ci_high)` for an interval bound.
    pub interval: Option<(f64, f64)>,
}

/// What one evaluation found: the falsifier's value, its outcome under HYP-21,
/// and the record HYP-22 and HYP-28 read.
#[derive(Debug, Clone, PartialEq)]
pub struct SliceEval {
    /// The falsifier under Kleene's logic: `Some(true)` refutes.
    pub value: Option<bool>,
    pub outcome: Outcome,
    /// Every sub-expression, keyed by its rendering and the cell (index into
    /// [`SliceData::cells`]) whose free parameters it was evaluated at.
    pub values: BTreeMap<(String, Option<usize>), Observed>,
    /// Every term read, keyed by the term's rendering, the context cell and,
    /// for a `noise_floor` per control, the control.
    pub readings: BTreeMap<(String, Option<usize>, Option<usize>), Reading>,
    /// HYP-22's decision cells (indices into [`SliceData::cells`]).
    pub decision_cells: BTreeSet<usize>,
}

/// HYP-21's reading of a falsifier's value.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum Outcome {
    /// True: the hypothesis is refuted, whatever else is undefined.
    Refuted,
    /// False with no undefined operand anywhere: the slice may pass.
    NotRefuted,
    /// Undefined, or false only past an undefined operand: inconclusive.
    Undefined,
}

#[derive(Debug, Default)]
struct Trace {
    values: BTreeMap<(String, Option<usize>), Observed>,
    readings: BTreeMap<(String, Option<usize>, Option<usize>), Reading>,
    decision: BTreeSet<usize>,
    /// The cells read directly in each open scope: the predicate's top level, or
    /// one context of an aggregate or `at` clause.
    scopes: Vec<BTreeSet<usize>>,
    undefined: bool,
}

/// One evaluation of a predicate over a slice. The bootstraps it draws are kept
/// so that both bounds of an interval, and every `ci`, read one set of
/// resamples (HYP-15).
pub struct Evaluation<'a> {
    slice: &'a SliceData,
    seed: u64,
    /// Every fix of every arm term; with a control term each parameter has one
    /// value (ADR-18), which is the control's cell.
    all_fixes: BTreeMap<String, String>,
    /// One representative cell per assignment of the free parameters, in order.
    contexts: Vec<usize>,
    stats: RefCell<BTreeMap<String, Option<Rc<Vec<f64>>>>>,
    trace: RefCell<Trace>,
}

impl<'a> Evaluation<'a> {
    /// Prepare to evaluate `predicate` over `slice` with the verdict seed `seed`
    /// ([`bootstrap::verdict_seed`]). Every sub-stream the predicate can draw
    /// from is derived here, so that a name the stream derivation refuses is an
    /// error, never an undefined value.
    pub fn new(predicate: &Expr, slice: &'a SliceData, seed: u64) -> Result<Self, SliceError> {
        let mut ts = Vec::new();
        eval::terms(predicate, &mut ts);
        let mut all_fixes = BTreeMap::new();
        for t in &ts {
            match t {
                Term::Arm { fixes, .. } => {
                    for (p, v) in fixes {
                        all_fixes.insert(p.clone(), v.clone());
                    }
                }
                Term::CiLow { quantity, .. } | Term::CiHigh { quantity, .. } => {
                    for c in &slice.cells {
                        let name = bootstrap::stream_name(
                            &slice.key,
                            &key(&c.cell),
                            quantity,
                            Function::Effect,
                        );
                        bootstrap::stream(seed, &name).map_err(|e| SliceError(e.to_string()))?;
                    }
                }
                Term::NoiseFloor { quantity, .. } => {
                    for k in &slice.controls {
                        let name = bootstrap::stream_name(
                            &slice.key,
                            &key(&k.config),
                            quantity,
                            Function::NoiseFloor,
                        );
                        bootstrap::stream(seed, &name).map_err(|e| SliceError(e.to_string()))?;
                    }
                }
                Term::Counter(_) => {}
            }
        }
        let mut seen = BTreeSet::new();
        let mut contexts = Vec::new();
        for (i, c) in slice.cells.iter().enumerate() {
            let free: Vec<String> = c
                .cell
                .iter()
                .filter(|(k, _)| !all_fixes.contains_key(*k))
                .map(|(k, v)| format!("{k}={}", v.text()))
                .collect();
            if seen.insert(free) {
                contexts.push(i);
            }
        }
        Ok(Self {
            slice,
            seed,
            all_fixes,
            contexts,
            stats: RefCell::new(BTreeMap::new()),
            trace: RefCell::new(Trace::default()),
        })
    }

    /// Evaluate the falsifier and return what was found. The cells read outside
    /// any aggregate or `at` clause decide the value too, so they are decision
    /// cells (ADR-19).
    #[must_use]
    pub fn run(&self, predicate: &Expr) -> SliceEval {
        *self.trace.borrow_mut() = Trace::default();
        self.push();
        let value = eval::truth(predicate, &self.at(self.contexts.first().copied()));
        let top = self.pop();
        let mut t = std::mem::take(&mut *self.trace.borrow_mut());
        t.decision.extend(top);
        let outcome = match value {
            Some(true) => Outcome::Refuted,
            Some(false) if !t.undefined => Outcome::NotRefuted,
            _ => Outcome::Undefined,
        };
        SliceEval {
            value,
            outcome,
            values: t.values,
            readings: t.readings,
            decision_cells: t.decision,
        }
    }

    /// A numeric expression's value at slice level.
    #[must_use]
    pub fn num(&self, e: &Expr) -> Option<f64> {
        eval::num(e, &self.at(self.contexts.first().copied()))
    }

    fn at(&self, cell: Option<usize>) -> Ctx<'_, 'a> {
        Ctx { ev: self, cell }
    }

    fn push(&self) {
        self.trace.borrow_mut().scopes.push(BTreeSet::new());
    }

    fn pop(&self) -> BTreeSet<usize> {
        self.trace.borrow_mut().scopes.pop().unwrap_or_default()
    }

    fn touch(&self, cells: impl IntoIterator<Item = usize>) {
        if let Some(s) = self.trace.borrow_mut().scopes.last_mut() {
            s.extend(cells);
        }
    }

    fn read(&self, k: (String, Option<usize>, Option<usize>), r: Reading) {
        let mut t = self.trace.borrow_mut();
        if r.value.is_none() {
            t.undefined = true;
        }
        t.readings.insert(k, r);
    }

    /// The cell matching cell `base` on every parameter `fixes` does not name,
    /// and `fixes` on the others.
    fn find(&self, base: Option<usize>, fixes: &BTreeMap<String, String>) -> Option<usize> {
        let base = &self.slice.cells.get(base?)?.cell;
        self.slice.cells.iter().position(|c| {
            c.cell.iter().all(|(p, v)| match fixes.get(p) {
                Some(text) => v.matches(text),
                None => base.get(p) == Some(v),
            })
        })
    }

    fn sorted_stats(
        &self,
        name: &str,
        make: impl FnOnce() -> Option<Vec<f64>>,
    ) -> Option<Rc<Vec<f64>>> {
        if let Some(s) = self.stats.borrow().get(name) {
            return s.clone();
        }
        let s = make().map(Rc::new);
        self.stats.borrow_mut().insert(name.to_owned(), s.clone());
        s
    }

    fn effect_stats(&self, cell: &CellData, q: &str) -> Option<Rc<Vec<f64>>> {
        let name = bootstrap::stream_name(&self.slice.key, &key(&cell.cell), q, Function::Effect);
        self.sorted_stats(&name, || {
            let t = cell.treatment.as_ref()?.values(q)?;
            let c = self.slice.controls.get(cell.control?)?.arm.values(q)?;
            // Derived once in `new`: the name is valid.
            bootstrap::effect_stats(&t, &c, bootstrap::stream(self.seed, &name).ok()?)
        })
    }

    /// The bounds at `ci` of `effect(q)` in cell `cell` (an index into
    /// [`SliceData::cells`]), from the same cached resamples the falsifier's
    /// `ci_low` and `ci_high` read (HYP-15). `None` when undefined.
    #[must_use]
    pub fn effect_interval(&self, cell: usize, q: &str, ci: f64) -> Option<(f64, f64)> {
        let cd = self.slice.cells.get(cell)?;
        bootstrap::bounds(&self.effect_stats(cd, q)?, ci)
    }

    /// Derive the `effect` sub-stream of `q` in every cell, so that a name the
    /// derivation refuses is an error, never an undefined interval.
    pub fn check_effect_streams(&self, q: &str) -> Result<(), SliceError> {
        for c in &self.slice.cells {
            let name = bootstrap::stream_name(&self.slice.key, &key(&c.cell), q, Function::Effect);
            bootstrap::stream(self.seed, &name).map_err(|e| SliceError(e.to_string()))?;
        }
        Ok(())
    }

    /// The largest split-half half-width over the slice's controls (HYP-13),
    /// recording each control's.
    fn noise_floor(&self, t: &Term, q: &str, ci: f64, ctx: Option<usize>) -> Option<f64> {
        let mut floor: Option<f64> = None;
        let mut defined = !self.slice.controls.is_empty();
        for (i, k) in self.slice.controls.iter().enumerate() {
            let name =
                bootstrap::stream_name(&self.slice.key, &key(&k.config), q, Function::NoiseFloor);
            let w = self
                .sorted_stats(&name, || {
                    bootstrap::split_half_stats(
                        &k.arm.values(q)?,
                        bootstrap::stream(self.seed, &name).ok()?,
                    )
                })
                .and_then(|s| bootstrap::half_width(&s, ci));
            self.read(
                (t.to_string(), ctx, Some(i)),
                Reading {
                    cell: None,
                    control: Some(i),
                    value: w,
                    completed: Some(k.arm.completed()),
                    interval: None,
                },
            );
            match w {
                Some(w) => floor = Some(floor.map_or(w, |f: f64| f.max(w))),
                None => defined = false,
            }
        }
        if defined { floor } else { None }
    }
}

/// The evaluation at one cell's free parameters (or at slice level, when the
/// slice has no cell).
struct Ctx<'e, 'a> {
    ev: &'e Evaluation<'a>,
    cell: Option<usize>,
}

impl Ctx<'_, '_> {
    fn arm_term(
        &self,
        t: &Term,
        quantity: &str,
        control: bool,
        fixes: &[(String, String)],
    ) -> Option<f64> {
        let ev = self.ev;
        let mut want: BTreeMap<String, String> = if control {
            ev.all_fixes.clone()
        } else {
            BTreeMap::new()
        };
        want.extend(fixes.iter().cloned());
        let found = ev.find(self.cell, &want);
        let cd = found.and_then(|i| ev.slice.cells.get(i));
        let (arm, k) = if control {
            let k = cd.and_then(|c| c.control);
            (k.and_then(|k| ev.slice.controls.get(k)).map(|k| &k.arm), k)
        } else {
            (cd.and_then(|c| c.treatment.as_ref()), None)
        };
        let value = arm.and_then(|a| a.mean(quantity));
        ev.touch(found);
        ev.read(
            (t.to_string(), self.cell, None),
            Reading {
                cell: found,
                control: k,
                value,
                completed: arm.map(Arm::completed),
                interval: None,
            },
        );
        value
    }
}

impl Source for Ctx<'_, '_> {
    fn term(&self, t: &Term) -> Option<f64> {
        let ev = self.ev;
        match t {
            Term::Arm {
                quantity,
                control,
                fixes,
            } => self.arm_term(t, quantity, *control, fixes),
            Term::CiLow { quantity, ci } | Term::CiHigh { quantity, ci } => {
                let cd = self.cell.and_then(|i| ev.slice.cells.get(i));
                let interval = cd
                    .and_then(|c| ev.effect_stats(c, quantity))
                    .and_then(|s| bootstrap::bounds(&s, f64::from_bits(*ci)));
                let value = interval.map(|(lo, hi)| {
                    if matches!(t, Term::CiLow { .. }) {
                        lo
                    } else {
                        hi
                    }
                });
                ev.touch(self.cell);
                ev.read(
                    (t.to_string(), self.cell, None),
                    Reading {
                        cell: self.cell,
                        control: cd.and_then(|c| c.control),
                        value,
                        completed: cd.and_then(|c| c.treatment.as_ref()).map(Arm::completed),
                        interval,
                    },
                );
                value
            }
            Term::NoiseFloor { quantity, ci } => {
                ev.noise_floor(t, quantity, f64::from_bits(*ci), self.cell)
            }
            Term::Counter(CounterKind::Replicates) => {
                ev.slice.min_replicates().map(bootstrap::as_f64)
            }
            // Load refuses it in a predicate (HYP-12); the guard is file-level.
            Term::Counter(CounterKind::ProvidersReported) => None,
        }
    }

    /// Strict: undefined if any context is, but every context is visited so
    /// that each is recorded.
    fn over_knobs(&self, max: bool, x: &Expr) -> Option<f64> {
        let ev = self.ev;
        let mut results = Vec::with_capacity(ev.contexts.len());
        for c in &ev.contexts {
            ev.push();
            let v = eval::num(x, &ev.at(Some(*c)));
            results.push((v, ev.pop()));
        }
        let mut best: Option<f64> = None;
        let mut defined = !results.is_empty();
        for (v, _) in &results {
            match v {
                Some(v) => {
                    best = Some(best.map_or(*v, |b| if max { b.max(*v) } else { b.min(*v) }))
                }
                None => defined = false,
            }
        }
        let best = if defined { best } else { None };
        // The cells an aggregate reads are its own decision; they are not read
        // directly by the expression around it.
        for (v, cells) in results {
            if best.is_some() && v == best {
                ev.trace.borrow_mut().decision.extend(cells);
            }
        }
        best
    }

    /// Kleene over the cells in range (HYP-11, HYP-14), each visited. Decision
    /// cells (HYP-22): every cell in range when `at all` holds or `at any`
    /// fails, otherwise the first deciding cell in HYP-14 order.
    fn quantify(&self, all: bool, range: &AtRange, inner: &Expr) -> Option<bool> {
        let ev = self.ev;
        let mut results = Vec::new();
        for c in &ev.contexts {
            if let AtRange::Bound(p, op, bound) = range {
                let v = ev
                    .slice
                    .cells
                    .get(*c)
                    .and_then(|cd| cd.cell.get(p))
                    .and_then(Value::num);
                if !v.is_some_and(|v| op.apply(v, *bound)) {
                    continue;
                }
            }
            ev.push();
            let b = eval::truth(inner, &ev.at(Some(*c)));
            results.push((b, ev.pop()));
        }
        if results.is_empty() {
            ev.trace.borrow_mut().undefined = true;
            return None;
        }
        // `at all` is decided by a false cell, `at any` by a true one.
        let decider = !all;
        let value = if results.iter().any(|(b, _)| *b == Some(decider)) {
            Some(decider)
        } else if results.iter().all(|(b, _)| b.is_some()) {
            Some(all)
        } else {
            None
        };
        let first = results.iter().position(|(b, _)| *b == Some(decider));
        for (i, (_, cells)) in results.into_iter().enumerate() {
            if first.is_none() || first == Some(i) {
                ev.trace.borrow_mut().decision.extend(cells);
            }
        }
        value
    }

    fn observe_num(&self, e: &Expr, v: Option<f64>) {
        let mut t = self.ev.trace.borrow_mut();
        if v.is_none() {
            t.undefined = true;
        }
        t.values
            .insert((e.to_string(), self.cell), Observed::Num(v));
    }

    fn observe_bool(&self, e: &Expr, v: Option<bool>) {
        let mut t = self.ev.trace.borrow_mut();
        if v.is_none() {
            t.undefined = true;
        }
        t.values
            .insert((e.to_string(), self.cell), Observed::Bool(v));
    }
}

/// The falsifier `predicate` over `slice` (HYP-11, HYP-21).
pub fn evaluate(predicate: &Expr, slice: &SliceData, seed: u64) -> Result<SliceEval, SliceError> {
    Ok(Evaluation::new(predicate, slice, seed)?.run(predicate))
}
