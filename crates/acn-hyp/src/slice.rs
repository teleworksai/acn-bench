//! A falsifier evaluated over one slice's data (HYP-11..14). The data are the
//! per-replicate values of each quantity, per arm and cell, which the verdict
//! engine reads from bundles; here they are evaluated by the one evaluator of
//! [`crate::eval`], so lint's probes and real verdicts share every rule.

use std::cell::RefCell;
use std::cmp::Ordering;
use std::collections::{BTreeMap, BTreeSet};

use acn_trace::identity::float_text;

use crate::bootstrap::{self, Function};
use crate::eval::{self, CounterKind, Source, Term};
use crate::file::{Domain, Hypothesis};
use crate::predicate::{AtRange, Expr};

/// A parameter's value in one cell (HYP-6), as the manifest's `vary.<name>` holds it.
#[derive(Debug, Clone, PartialEq)]
pub enum Value {
    Bool(bool),
    Enum(String),
    Int(i64),
    /// Finite: [`Value::float`] refuses NaN and the infinities.
    Float(f64),
}

impl Value {
    /// A `range` value; `None` unless finite (CON-27(c)).
    #[must_use]
    pub fn float(v: f64) -> Option<Self> {
        v.is_finite().then_some(Self::Float(v))
    }

    /// The text form of CON-27(c).
    #[must_use]
    pub fn text(&self) -> String {
        match self {
            Self::Bool(b) => b.to_string(),
            Self::Enum(s) => s.clone(),
            Self::Int(i) => i.to_string(),
            Self::Float(f) => float_text(*f).unwrap_or_default(),
        }
    }

    /// The value as a number, for an `at` bound.
    #[must_use]
    pub fn num(&self) -> Option<f64> {
        match self {
            #[allow(clippy::cast_precision_loss)] // int_range values are checked to be exact
            Self::Int(i) => Some(*i as f64),
            Self::Float(f) => Some(*f),
            Self::Bool(_) | Self::Enum(_) => None,
        }
    }

    /// Whether a selector's value (as [`crate::eval::lit_text`] writes it) names this value.
    #[must_use]
    pub fn matches(&self, text: &str) -> bool {
        match self {
            Self::Bool(b) => text == if *b { "true" } else { "false" },
            Self::Enum(s) => text == s,
            Self::Int(_) | Self::Float(_) => text.parse::<f64>().ok() == self.num(),
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
/// numbers, `false` before `true`, enum values in declaration order.
#[must_use]
pub fn cell_order(h: &Hypothesis, a: &Cell, b: &Cell) -> Ordering {
    for ((name, va), (_, vb)) in a.iter().zip(b) {
        let o = match (va, vb) {
            (Value::Bool(x), Value::Bool(y)) => x.cmp(y),
            (Value::Enum(x), Value::Enum(y)) => {
                let pos = |v: &String| match h.params.get(name).map(|p| &p.domain) {
                    Some(Domain::Enum(vals)) => vals.iter().position(|w| w == v),
                    _ => None,
                };
                pos(x).cmp(&pos(y)).then_with(|| x.cmp(y))
            }
            _ => match (va.num(), vb.num()) {
                (Some(x), Some(y)) => x.total_cmp(&y),
                _ => Ordering::Equal,
            },
        };
        if o != Ordering::Equal {
            return o;
        }
    }
    a.len().cmp(&b.len())
}

/// One replicate's values: quantity → value, `None` when undefined (HYP-11).
pub type Values = BTreeMap<String, Option<f64>>;

/// One arm of one cell: the replicates with index `0..[design].replicates`,
/// each `None` unless it completed.
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

    /// Every replicate's value of `q`, in index order; `None` unless all `n`
    /// completed and each value is defined (HYP-11: an undefined replicate makes
    /// the arm undefined, and is never skipped).
    #[must_use]
    pub fn values(&self, q: &str, n: usize) -> Option<Vec<f64>> {
        if n == 0 || self.replicates.len() != n {
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
    pub fn mean(&self, q: &str, n: usize) -> Option<f64> {
        let v = self.values(q, n)?;
        let mut sum = 0.0;
        for x in &v {
            sum += x;
        }
        #[allow(clippy::cast_precision_loss)]
        let n = n as f64;
        Some(sum / n).filter(|m| m.is_finite())
    }
}

/// One treatment cell of the slice.
#[derive(Debug, Clone, PartialEq)]
pub struct CellData {
    pub cell: Cell,
    /// `None` for a grid cell with no runs (undefined, HYP-21).
    pub treatment: Option<Arm>,
    /// The index in [`SliceData::controls`] of the control arm it maps to (HYP-8).
    pub control: Option<usize>,
}

/// One control arm, keyed by its effective configuration (HYP-8).
#[derive(Debug, Clone, PartialEq)]
pub struct ControlData {
    pub config: Cell,
    pub arm: Arm,
}

/// Everything one slice's verdict reads.
#[derive(Debug, Clone, PartialEq)]
pub struct SliceData {
    /// The slice key (HYP-15); empty for a file with one slice.
    pub key: String,
    /// `[design].replicates`.
    pub replicates: usize,
    /// Every cell of the slice in HYP-14 order: for a grid, every grid cell.
    pub cells: Vec<CellData>,
    pub controls: Vec<ControlData>,
}

impl SliceData {
    /// The counter `replicates` within the slice (HYP-12): the minimum over its
    /// evaluated cells and both arms of the completed replicates; a missing
    /// control counts as none.
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

/// One evaluation of a predicate over a slice: the bootstraps it has drawn are
/// kept so that both bounds of an interval, and every `ci`, read one set of
/// resamples (HYP-15).
pub struct Evaluation<'a> {
    slice: &'a SliceData,
    seed: u64,
    /// Parameters some `select` fixes: aggregates and `at` do not range over them.
    fixed: BTreeSet<String>,
    /// Every fix of every arm term; with a control term each parameter has one
    /// value (ADR-18), which is the control's cell.
    all_fixes: BTreeMap<String, String>,
    /// One representative cell per assignment of the free parameters, in order.
    contexts: Vec<usize>,
    stats: RefCell<BTreeMap<String, Option<Vec<f64>>>>,
}

impl<'a> Evaluation<'a> {
    /// Prepare to evaluate `predicate` over `slice` with the verdict seed `seed`
    /// ([`bootstrap::verdict_seed`]).
    #[must_use]
    pub fn new(predicate: &Expr, slice: &'a SliceData, seed: u64) -> Self {
        let mut ts = Vec::new();
        eval::terms(predicate, &mut ts);
        let mut all_fixes = BTreeMap::new();
        for t in &ts {
            if let Term::Arm { fixes, .. } = t {
                for (p, v) in fixes {
                    all_fixes.insert(p.clone(), v.clone());
                }
            }
        }
        let fixed: BTreeSet<String> = all_fixes.keys().cloned().collect();
        let mut seen = BTreeSet::new();
        let mut contexts = Vec::new();
        for (i, c) in slice.cells.iter().enumerate() {
            let free: Vec<String> = c
                .cell
                .iter()
                .filter(|(k, _)| !fixed.contains(*k))
                .map(|(k, v)| format!("{k}={}", v.text()))
                .collect();
            if seen.insert(free) {
                contexts.push(i);
            }
        }
        Self {
            slice,
            seed,
            fixed,
            all_fixes,
            contexts,
            stats: RefCell::new(BTreeMap::new()),
        }
    }

    /// The falsifier's value: `Some(true)` refutes (HYP-10), `None` is undefined.
    #[must_use]
    pub fn truth(&self, predicate: &Expr) -> Option<bool> {
        eval::truth(predicate, &self.at(self.contexts.first().copied()))
    }

    /// A numeric expression's value at slice level.
    #[must_use]
    pub fn num(&self, e: &Expr) -> Option<f64> {
        eval::num(e, &self.at(self.contexts.first().copied()))
    }

    fn at(&self, cell: Option<usize>) -> Ctx<'_, 'a> {
        Ctx { ev: self, cell }
    }

    /// The parameters fixed by some selector.
    #[must_use]
    pub fn fixed(&self) -> &BTreeSet<String> {
        &self.fixed
    }

    /// The cell matching `base` on every parameter `fixes` does not name, and
    /// `fixes` on the others.
    fn find(&self, base: Option<usize>, fixes: &BTreeMap<String, String>) -> Option<&CellData> {
        let base = &self.slice.cells.get(base?)?.cell;
        self.slice.cells.iter().find(|c| {
            c.cell.len() == base.len()
                && c.cell.iter().all(|(p, v)| match fixes.get(p) {
                    Some(text) => v.matches(text),
                    None => base.get(p) == Some(v),
                })
        })
    }

    fn sorted_stats(
        &self,
        name: String,
        make: impl FnOnce() -> Option<Vec<f64>>,
    ) -> Option<Vec<f64>> {
        if let Some(s) = self.stats.borrow().get(&name) {
            return s.clone();
        }
        let s = make();
        self.stats.borrow_mut().insert(name, s.clone());
        s
    }

    fn effect_stats(&self, cell: &CellData, q: &str) -> Option<Vec<f64>> {
        let n = self.slice.replicates;
        let name = bootstrap::stream_name(&self.slice.key, &key(&cell.cell), q, Function::Effect);
        let seed = self.seed;
        self.sorted_stats(name.clone(), || {
            let t = cell.treatment.as_ref()?.values(q, n)?;
            let c = self.slice.controls.get(cell.control?)?.arm.values(q, n)?;
            bootstrap::effect_stats(&t, &c, bootstrap::stream(seed, &name).ok()?)
        })
    }

    fn noise_floor(&self, q: &str, ci: f64) -> Option<f64> {
        let n = self.slice.replicates;
        let mut floor: Option<f64> = None;
        for k in &self.slice.controls {
            let name =
                bootstrap::stream_name(&self.slice.key, &key(&k.config), q, Function::NoiseFloor);
            let seed = self.seed;
            let stats = self.sorted_stats(name.clone(), || {
                bootstrap::split_half_stats(
                    &k.arm.values(q, n)?,
                    bootstrap::stream(seed, &name).ok()?,
                )
            })?;
            let w = bootstrap::half_width(&stats, ci)?;
            floor = Some(floor.map_or(w, |f: f64| f.max(w)));
        }
        floor
    }
}

/// The evaluation at one cell (or at slice level, when the slice has no cell).
struct Ctx<'e, 'a> {
    ev: &'e Evaluation<'a>,
    cell: Option<usize>,
}

impl Source for Ctx<'_, '_> {
    fn term(&self, t: &Term) -> Option<f64> {
        let ev = self.ev;
        let n = ev.slice.replicates;
        match t {
            Term::Arm {
                quantity,
                control,
                fixes,
            } => {
                let own: BTreeMap<String, String> = fixes.iter().cloned().collect();
                if *control {
                    let mut all = ev.all_fixes.clone();
                    all.extend(own);
                    let c = ev.find(self.cell, &all)?;
                    ev.slice.controls.get(c.control?)?.arm.mean(quantity, n)
                } else {
                    ev.find(self.cell, &own)?
                        .treatment
                        .as_ref()?
                        .mean(quantity, n)
                }
            }
            Term::CiLow { quantity, ci } | Term::CiHigh { quantity, ci } => {
                let cell = ev.slice.cells.get(self.cell?)?;
                let stats = ev.effect_stats(cell, quantity)?;
                let (lo, hi) = bootstrap::bounds(&stats, f64::from_bits(*ci))?;
                Some(if matches!(t, Term::CiLow { .. }) {
                    lo
                } else {
                    hi
                })
            }
            Term::NoiseFloor { quantity, ci } => ev.noise_floor(quantity, f64::from_bits(*ci)),
            Term::Counter(CounterKind::Replicates) =>
            {
                #[allow(clippy::cast_precision_loss)]
                ev.slice.min_replicates().map(|r| r as f64)
            }
            Term::Counter(CounterKind::ProvidersReported) => None,
        }
    }

    fn over_knobs(&self, max: bool, x: &Expr) -> Option<f64> {
        let mut out: Option<f64> = None;
        for c in &self.ev.contexts {
            let v = eval::num(x, &self.ev.at(Some(*c)))?;
            out = Some(match out {
                None => v,
                Some(o) if max => o.max(v),
                Some(o) => o.min(v),
            });
        }
        out
    }

    fn quantify(&self, all: bool, range: &AtRange, inner: &Expr) -> Option<bool> {
        let mut any_cell = false;
        let mut undefined = false;
        for c in &self.ev.contexts {
            if let AtRange::Bound(p, op, bound) = range {
                let v = self.ev.slice.cells.get(*c)?.cell.get(p)?.num()?;
                if !op.apply(v, *bound) {
                    continue;
                }
            }
            any_cell = true;
            match eval::truth(inner, &self.ev.at(Some(*c))) {
                Some(b) if b != all => return Some(b),
                Some(_) => {}
                None => undefined = true,
            }
        }
        // `at all`: false as soon as one cell is false; `at any`: true as soon as
        // one is true (Kleene, HYP-11). No cell in range: undefined (HYP-14).
        (any_cell && !undefined).then_some(all)
    }
}

/// The falsifier `predicate` over `slice`: `Some(true)` refutes, `Some(false)`
/// does not, `None` is undefined (HYP-11, HYP-21).
#[must_use]
pub fn evaluate(predicate: &Expr, slice: &SliceData, seed: u64) -> Option<bool> {
    Evaluation::new(predicate, slice, seed).truth(predicate)
}
