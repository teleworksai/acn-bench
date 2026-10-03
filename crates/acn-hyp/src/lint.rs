//! `acn hyp lint` (HYP-27): parse, type-check and resolve a file without reading
//! any bundle, and, for a frozen file, insist that the falsifier reads a primary
//! quantity and can both fire and fail to fire. The witness search evaluates the
//! predicate in one probe cell, each distinct quantity term (per arm, the same in
//! every cell), each `noise_floor` and each interval bound drawn from
//! {0, 1e-9, 1, 1e9}, with `ci_low ≤ ci_high`, under HYP-11's three-valued
//! logic.

use std::collections::BTreeMap;
use std::path::Path;

use crate::check::{cells_per_slice, uses};
use crate::file::{Hypothesis, load};
use crate::predicate::{Arg, ArithOp, AtRange, Counter, Expr, Selector};
use crate::{HypError, Status};

/// The probe values of HYP-27.
pub const PROBES: [f64; 4] = [0.0, 1e-9, 1.0, 1e9];

/// The most probe slots searched exhaustively (4^10 evaluations).
pub const MAX_SLOTS: usize = 10;

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
    pub fires: Option<BTreeMap<String, f64>>,
    pub holds: Option<BTreeMap<String, f64>>,
}

impl Report {
    #[must_use]
    pub fn ok(&self) -> bool {
        self.errors.is_empty()
    }
}

fn arm_key(sels: &[Selector]) -> String {
    let mut parts: Vec<String> = sels
        .iter()
        .filter_map(|s| match s {
            Selector::Control | Selector::Treatment => None,
            Selector::Fix(p, v) => Some(format!("{p}={v}")),
            Selector::Value(v) => Some(v.clone()),
        })
        .collect();
    parts.sort();
    let arm = if sels.contains(&Selector::Control) {
        "control"
    } else {
        "treatment"
    };
    format!("{arm}[{}]", parts.join(","))
}

/// The probe slots a predicate reads, in a fixed order.
fn slots(e: &Expr, out: &mut Vec<String>) {
    let mut add = |k: String| {
        if !out.contains(&k) {
            out.push(k);
        }
    };
    match e {
        Expr::Num(_) => {}
        Expr::Counter(_) => add("replicates".into()),
        Expr::Quantity(q) => add(format!("{q}:treatment[]")),
        Expr::Select(q, sels) => add(format!("{q}:{}", arm_key(sels))),
        Expr::Call(f, args) => {
            let q = match args.first() {
                Some(Arg::Expr(Expr::Quantity(q))) => q.clone(),
                _ => String::new(),
            };
            match f.as_str() {
                "effect" | "rel_effect" => {
                    add(format!("{q}:treatment[]"));
                    add(format!("{q}:control[]"));
                }
                "ci_low" | "ci_high" => {
                    add(format!("{q}:ci_low"));
                    add(format!("{q}:ci_high"));
                }
                "noise_floor" => add(format!("{q}:noise_floor")),
                _ => {
                    for a in args {
                        if let Arg::Expr(x) = a {
                            slots(x, out);
                        }
                    }
                }
            }
        }
        Expr::Neg(x) | Expr::Not(x) | Expr::At(x, _, _) => slots(x, out),
        Expr::Arith(_, a, b) | Expr::Cmp(_, a, b) | Expr::And(a, b) | Expr::Or(a, b) => {
            slots(a, out);
            slots(b, out);
        }
    }
}

type V = Option<f64>;

fn num(e: &Expr, env: &BTreeMap<String, f64>) -> V {
    let get = |k: String| env.get(&k).copied();
    let v = match e {
        Expr::Num(n) => Some(*n),
        Expr::Counter(Counter::Replicates) => get("replicates".into()),
        Expr::Counter(Counter::ProvidersReported) => None,
        Expr::Quantity(q) => get(format!("{q}:treatment[]")),
        Expr::Select(q, sels) => get(format!("{q}:{}", arm_key(sels))),
        Expr::Neg(x) => num(x, env).map(|v| -v),
        Expr::Arith(op, a, b) => {
            let (a, b) = (num(a, env)?, num(b, env)?);
            match op {
                ArithOp::Add => Some(a + b),
                ArithOp::Sub => Some(a - b),
                ArithOp::Mul => Some(a * b),
                ArithOp::Div if b == 0.0 => None,
                ArithOp::Div => Some(a / b),
            }
        }
        Expr::Call(f, args) => {
            let q = match args.first() {
                Some(Arg::Expr(Expr::Quantity(q))) => q.clone(),
                _ => String::new(),
            };
            let arg = |i: usize| match args.get(i) {
                Some(Arg::Expr(x)) => num(x, env),
                _ => None,
            };
            match f.as_str() {
                "abs" => arg(0).map(f64::abs),
                "min" => Some(arg(0)?.min(arg(1)?)),
                "max" => Some(arg(0)?.max(arg(1)?)),
                "max_over_knobs" | "min_over_knobs" => arg(0),
                "effect" => Some(get(format!("{q}:treatment[]"))? - get(format!("{q}:control[]"))?),
                "rel_effect" => {
                    let c = get(format!("{q}:control[]"))?;
                    if c == 0.0 {
                        None
                    } else {
                        Some((get(format!("{q}:treatment[]"))? - c) / c)
                    }
                }
                "ci_low" => get(format!("{q}:ci_low")),
                "ci_high" => get(format!("{q}:ci_high")),
                // HYP-13: a zero noise floor is undefined.
                "noise_floor" => get(format!("{q}:noise_floor")).filter(|v| *v != 0.0),
                _ => None,
            }
        }
        _ => None,
    };
    v.filter(|x| x.is_finite())
}

/// Kleene's three-valued logic (HYP-11), in one cell.
fn truth(e: &Expr, env: &BTreeMap<String, f64>) -> Option<bool> {
    match e {
        Expr::Cmp(op, a, b) => Some(op.apply(num(a, env)?, num(b, env)?)),
        Expr::And(a, b) => match (truth(a, env), truth(b, env)) {
            (Some(false), _) | (_, Some(false)) => Some(false),
            (Some(true), Some(true)) => Some(true),
            _ => None,
        },
        Expr::Or(a, b) => match (truth(a, env), truth(b, env)) {
            (Some(true), _) | (_, Some(true)) => Some(true),
            (Some(false), Some(false)) => Some(false),
            _ => None,
        },
        Expr::Not(x) => truth(x, env).map(|b| !b),
        // One probe cell, taken to satisfy the bound.
        Expr::At(x, _, _) => truth(x, env),
        _ => None,
    }
}

/// One probe assignment: slot name → value.
pub type Probe = BTreeMap<String, f64>;

/// Search the probe assignments for one under which the predicate is true and
/// one under which it is false (HYP-27); `None` when there are too many slots.
#[must_use]
pub fn witnesses(predicate: &Expr) -> Option<(Option<Probe>, Option<Probe>)> {
    let mut keys = Vec::new();
    slots(predicate, &mut keys);
    if keys.len() > MAX_SLOTS {
        return None;
    }
    let (mut fires, mut holds) = (None, None);
    let n = keys.len() as u32;
    for code in 0..PROBES.len().pow(n) {
        let mut env = BTreeMap::new();
        let mut c = code;
        for k in &keys {
            env.insert(k.clone(), PROBES[c % PROBES.len()]);
            c /= PROBES.len();
        }
        let consistent = env.iter().all(|(k, lo)| {
            k.strip_suffix(":ci_low")
                .and_then(|q| env.get(&format!("{q}:ci_high")))
                .is_none_or(|hi| lo <= hi)
        });
        if !consistent {
            continue;
        }
        match truth(predicate, &env) {
            Some(true) if fires.is_none() => fires = Some(env),
            Some(false) if holds.is_none() => holds = Some(env),
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
                    Arg::Expr(x) => has(x, pred),
                    _ => false,
                }),
                _ => false,
            }
    }
    let point = |x: &Expr| {
        matches!(x, Expr::Quantity(_) | Expr::Select(..))
            || matches!(x, Expr::Call(f, _) if f == "effect" || f == "rel_effect")
    };
    let guarded = |x: &Expr| matches!(x, Expr::Call(f, _) if f == "ci_low" || f == "ci_high" || f == "noise_floor");
    match e {
        Expr::Cmp(..) => has(e, &point) && !has(e, &guarded),
        Expr::Not(x) | Expr::At(x, _, _) => point_comparison(x),
        Expr::And(a, b) | Expr::Or(a, b) => point_comparison(a) || point_comparison(b),
        _ => false,
    }
}

/// Lint a loaded file.
#[must_use]
pub fn lint_hypothesis(h: &Hypothesis) -> Report {
    let mut r = Report {
        id: Some(h.id.clone()),
        status: Some(h.status),
        hash: Some(h.hash.to_hex()),
        predicate: Some(h.predicate.to_string()),
        guard: h.guard.as_ref().map(ToString::to_string),
        warnings: h.warnings.clone(),
        ..Report::default()
    };
    let frozen = h.status == Status::Frozen;
    // A frozen file's failures are errors; a candidate's are reported (HYP-27).
    let mut finding = |m: String| {
        if frozen {
            r.errors.push(m);
        } else {
            r.warnings.push(m);
        }
    };
    let Ok((u, _)) = uses(h) else {
        return r;
    };
    if !u.quantities.iter().any(|q| h.primary.contains(q)) {
        finding("the falsifier reads no primary quantity (HYP-27)".into());
    }
    match witnesses(&h.predicate) {
        None => finding(format!(
            "the predicate reads more than {MAX_SLOTS} distinct terms; no witness search was made"
        )),
        Some((fires, holds)) => {
            if fires.is_none() {
                finding(
                    "no probe values make the falsifier true: it can never fire (HYP-27)".into(),
                );
            }
            if holds.is_none() {
                finding(
                    "no probe values make the falsifier false: it always fires (HYP-27)".into(),
                );
            }
            r.fires = fires;
            r.holds = holds;
        }
    }
    if point_comparison(&h.predicate) {
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

/// Lint the file at `path` (HYP-27). A file that does not load is reported with
/// the reason, never partially linted.
#[must_use]
pub fn lint(path: &Path) -> Report {
    match load(path) {
        Ok(h) => lint_hypothesis(&h),
        Err(e) => Report {
            errors: vec![e.to_string()],
            ..Report::default()
        },
    }
}

impl From<HypError> for Report {
    fn from(e: HypError) -> Self {
        Self {
            errors: vec![e.to_string()],
            ..Self::default()
        }
    }
}
