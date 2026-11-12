//! `acn hyp lint` (HYP-27): parse, type-check and resolve a file without reading
//! any bundle, and, for a frozen file, insist that the falsifier reads a primary
//! quantity, can both fire and fail to fire, and is not disarmed by its guard.
//! The witness search evaluates the predicate with the shared evaluator
//! ([`crate::eval`]) in one probe cell: each distinct term (per arm, the same in
//! every cell) and each `noise_floor` is drawn from {0, 1e-9, 1, 1e9}, and each
//! interval bound, which bounds an effect and may be negative, from those values
//! and their negatives, with `ci_low ≤ ci_high`. `effect` and `rel_effect` are
//! not probed: the evaluator computes them from their arms' terms.

use std::collections::BTreeMap;
use std::path::Path;

use crate::Status;
use crate::check::{cells_per_slice, uses};
use crate::eval::{CounterKind, Source, Term, terms, truth};
use crate::file::{Hypothesis, load};
use crate::predicate::{AtRange, Builtin, Expr};

/// The probe values of HYP-27 for a quantity term and a `noise_floor`.
pub const PROBES: [f64; 4] = [0.0, 1e-9, 1.0, 1e9];

/// The probe values of an interval bound (HYP-27): those and their negatives,
/// since a bound of an effect may be negative.
pub const SIGNED_PROBES: [f64; 7] = [-1e9, -1.0, -1e-9, 0.0, 1e-9, 1.0, 1e9];

/// The most probe slots searched exhaustively.
pub const MAX_SLOTS: usize = 10;

/// The most assignments searched: ten unsigned slots' worth. A predicate with
/// many interval bounds reaches it with fewer slots.
pub const MAX_ASSIGNMENTS: u64 = 1 << 20;

/// One probe assignment, rendered: term → value.
pub type Probe = BTreeMap<String, f64>;

/// What lint found.
#[derive(Debug, Clone, Default)]
pub struct Report {
    pub id: Option<String>,
    pub status: Option<Status>,
    pub hash: Option<String>,
    pub predicate: Option<String>,
    pub guard: Option<String>,
    pub errors: Vec<String>,
    pub warnings: Vec<String>,
    /// Probe assignments under which the falsifier is true and false.
    pub fires: Option<Probe>,
    pub holds: Option<Probe>,
}

impl Report {
    #[must_use]
    pub fn ok(&self) -> bool {
        self.errors.is_empty()
    }
}

/// One probe cell: every term's value, by index.
struct Cell<'a> {
    index: &'a BTreeMap<Term, usize>,
    values: Vec<f64>,
}

impl Source for Cell<'_> {
    fn term(&self, t: &Term) -> Option<f64> {
        self.index.get(t).map(|i| self.values[*i])
    }

    fn over_knobs(&self, _max: bool, x: &Expr) -> Option<f64> {
        crate::eval::num(x, self)
    }

    /// One probe cell; load has checked that some cell satisfies the bound.
    fn quantify(&self, _all: bool, _range: &AtRange, inner: &Expr) -> Option<bool> {
        truth(inner, self)
    }
}

fn render(keys: &[Term], values: &[f64]) -> Probe {
    keys.iter()
        .zip(values)
        .map(|(k, v)| (k.to_string(), *v))
        .collect()
}

/// Search the probe assignments for one under which the predicate is true and
/// one under which it is false; `None` when there are too many to search. One
/// search, each slot over its own values (HYP-27): an interval bound over
/// [`SIGNED_PROBES`], every other term over [`PROBES`].
#[must_use]
pub fn witnesses(predicate: &Expr) -> Option<(Option<Probe>, Option<Probe>)> {
    let mut keys = Vec::new();
    terms(predicate, &mut keys);
    if keys.len() > MAX_SLOTS {
        return None;
    }
    let slots: Vec<&[f64]> = keys
        .iter()
        .map(|k| match k {
            Term::CiLow { .. } | Term::CiHigh { .. } => &SIGNED_PROBES[..],
            _ => &PROBES[..],
        })
        .collect();
    let total = slots
        .iter()
        .try_fold(1u64, |n, s| n.checked_mul(s.len() as u64))
        .filter(|n| *n <= MAX_ASSIGNMENTS)?;
    let index: BTreeMap<Term, usize> = keys.iter().cloned().zip(0..).collect();
    // Interval bounds pair up: low ≤ high.
    let pairs: Vec<(usize, usize)> = keys
        .iter()
        .enumerate()
        .filter_map(|(i, k)| match k {
            Term::CiLow { quantity, ci } => index
                .get(&Term::CiHigh {
                    quantity: quantity.clone(),
                    ci: *ci,
                })
                .map(|j| (i, *j)),
            _ => None,
        })
        .collect();
    // A noise floor is a spread: never negative, whatever the probes.
    let floors: Vec<usize> = keys
        .iter()
        .enumerate()
        .filter_map(|(i, k)| matches!(k, Term::NoiseFloor { .. }).then_some(i))
        .collect();
    let mut cell = Cell {
        index: &index,
        values: vec![0.0; keys.len()],
    };
    let (mut fires, mut holds) = (None, None);
    for code in 0..total {
        let mut c = code;
        for (v, probes) in cell.values.iter_mut().zip(&slots) {
            let n = probes.len() as u64;
            *v = probes[usize::try_from(c % n).ok()?];
            c /= n;
        }
        if pairs
            .iter()
            .any(|(lo, hi)| cell.values[*lo] > cell.values[*hi])
            || floors.iter().any(|i| cell.values[*i] < 0.0)
        {
            continue;
        }
        match truth(predicate, &cell) {
            Some(true) if fires.is_none() => fires = Some(render(&keys, &cell.values)),
            Some(false) if holds.is_none() => holds = Some(render(&keys, &cell.values)),
            _ => {}
        }
        if fires.is_some() && holds.is_some() {
            break;
        }
    }
    Some((fires, holds))
}

/// Whether a comparison reads a per-cell point estimate without an interval
/// bound or a noise floor (HYP-27's first warning).
fn point_comparison(e: &Expr) -> bool {
    fn has(e: &Expr, pred: &dyn Fn(&Expr) -> bool) -> bool {
        pred(e)
            || match e {
                Expr::Neg(x) | Expr::Not(x) | Expr::At(x, _, _) => has(x, pred),
                Expr::Arith(_, a, b) | Expr::Cmp(_, a, b) | Expr::And(a, b) | Expr::Or(a, b) => {
                    has(a, pred) || has(b, pred)
                }
                Expr::Call(_, args) => args.iter().any(|a| match a {
                    crate::predicate::Arg::Expr(x) => has(x, pred),
                    _ => false,
                }),
                _ => false,
            }
    }
    let point = |x: &Expr| {
        matches!(x, Expr::Quantity(_) | Expr::Select(..))
            || matches!(x, Expr::Call(Builtin::Effect | Builtin::RelEffect, _))
    };
    let guarded = |x: &Expr| {
        matches!(
            x,
            Expr::Call(Builtin::CiLow | Builtin::CiHigh | Builtin::NoiseFloor, _)
        )
    };
    match e {
        Expr::Cmp(..) => has(e, &point) && !has(e, &guarded),
        Expr::Not(x) | Expr::At(x, _, _) => point_comparison(x),
        Expr::And(a, b) | Expr::Or(a, b) => point_comparison(a) || point_comparison(b),
        _ => false,
    }
}

/// The guard's world once every replicate and every declared provider is in.
struct FullRun {
    replicates: f64,
    providers: Option<f64>,
}

impl Source for FullRun {
    fn term(&self, t: &Term) -> Option<f64> {
        match t {
            Term::Counter(CounterKind::Replicates) => Some(self.replicates),
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

fn finding(r: &mut Report, frozen: bool, m: String) {
    // A frozen file's failures are errors; a candidate's are reported (HYP-27).
    if frozen {
        r.errors.push(m);
    } else {
        r.warnings.push(m);
    }
}

/// Lint a loaded file.
#[must_use]
pub fn lint_hypothesis(h: &Hypothesis) -> Report {
    let mut r = Report {
        id: Some(h.id.clone()),
        status: Some(h.status()),
        hash: Some(h.hash().to_hex()),
        predicate: Some(h.predicate().to_string()),
        guard: h.guard().map(ToString::to_string),
        warnings: h.warnings.clone(),
        ..Report::default()
    };
    let frozen = h.status() == Status::Frozen;
    let (u, _) = match uses(h) {
        Ok(x) => x,
        Err(m) => {
            r.errors.push(m);
            return r;
        }
    };
    if !u.quantities.iter().any(|q| h.primary.contains(q)) {
        finding(
            &mut r,
            frozen,
            "the falsifier reads no primary quantity (HYP-27)".into(),
        );
    }
    match witnesses(h.predicate()) {
        None => finding(
            &mut r,
            frozen,
            format!(
                "the predicate reads more than {MAX_SLOTS} distinct terms, or too many interval bounds to search; no witness search was made"
            ),
        ),
        Some((fires, holds)) => {
            if fires.is_none() {
                finding(
                    &mut r,
                    frozen,
                    "no probe values make the falsifier true: it can never fire (HYP-27)".into(),
                );
            }
            if holds.is_none() {
                finding(
                    &mut r,
                    frozen,
                    "no probe values make the falsifier false: it always fires (HYP-27)".into(),
                );
            }
            r.fires = fires;
            r.holds = holds;
        }
    }
    // A guard that is true even when everything is in makes every verdict
    // inconclusive (HYP-24): the falsifier is disarmed (ADR-18).
    if let Some(g) = h.guard() {
        #[allow(clippy::cast_precision_loss)]
        let full = FullRun {
            replicates: f64::from(h.design.replicates),
            providers: h.provider_count().map(|n| n as f64),
        };
        if truth(g, &full) != Some(false) {
            finding(
                &mut r,
                frozen,
                "the guard is not false when every replicate and every declared provider is in: no verdict could ever be a pass or a fail (HYP-24, HYP-27)".into(),
            );
        }
    }
    if point_comparison(h.predicate()) {
        r.warnings.push(
            "a per-cell comparison uses neither an interval bound nor `noise_floor` (HYP-27)"
                .into(),
        );
    }
    if let Some((true, range)) = &u.at {
        let many = match range {
            AtRange::Cells => cells_per_slice(h) != Some(1),
            AtRange::Bound(p, _, _) => h.params.get(p).and_then(|x| x.domain.size()) != Some(1),
        };
        if many && !u.interval_or_floor {
            r.warnings.push(
                "`at all` ranges over more than one cell while comparing point estimates: one noisy cell decides (HYP-27)".into(),
            );
        }
    }
    r
}

fn report_of(loaded: Result<Hypothesis, crate::HypError>) -> Report {
    match loaded {
        Ok(h) => lint_hypothesis(&h),
        Err(e) => Report {
            errors: vec![e.to_string()],
            ..Report::default()
        },
    }
}

/// Lint the file at `path` (HYP-27), its status decided against the current
/// directory's workspace root. A file that does not load is reported with the
/// reason, never partially linted.
#[must_use]
pub fn lint(path: &Path) -> Report {
    report_of(load(path))
}

/// [`lint`] with the workspace root found from `start`.
#[must_use]
pub fn lint_in(path: &Path, start: &Path) -> Report {
    report_of(crate::file::load_in(path, start))
}
