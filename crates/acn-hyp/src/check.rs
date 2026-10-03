//! Static checks of the predicates (HYP-7, HYP-9, HYP-10..14): types, units,
//! the built-ins' signatures, the guard's restricted language, the selector
//! rules, and per-cell versus slice-level. A file that fails any of them fails to
//! load (HYP-10).

use std::collections::{BTreeMap, BTreeSet};

use crate::file::Hypothesis;
use crate::predicate::{Arg, AtRange, Counter, Expr, Lit, Selector};
use crate::{HypError, quantities};

/// A predicate type (HYP-11). A number carries a unit, or `None` when it is
/// compatible with any (a literal, a counter, the result of `*` or `/`, an
/// unresolved quantity).
#[derive(Debug, Clone, PartialEq, Eq)]
pub enum Ty {
    Num(Option<String>),
    Bool,
}

/// Where a sub-expression lives (HYP-14).
#[derive(Debug, Clone, Copy, PartialEq, Eq, PartialOrd, Ord)]
pub enum Level {
    Slice,
    PerCell,
}

/// What a walk of a predicate found.
#[derive(Debug, Default, Clone)]
pub struct Uses {
    /// Every quantity read, by any route.
    pub quantities: BTreeSet<String>,
    /// For each treatment-arm `select`, the parameters it fixes and to what.
    pub treatment_fixes: Vec<BTreeMap<String, Lit>>,
    /// Whether any `select` chooses the control arm.
    pub control_select: bool,
    /// Bare `qname`s and per-cell built-ins (`effect`, `rel_effect`, `ci_*`).
    pub free_terms: usize,
    pub interval_or_floor: bool,
    pub at: Option<(bool, AtRange)>,
    pub aggregated: bool,
}

struct Ctx<'a> {
    h: &'a Hypothesis,
    guard: bool,
    uses: Uses,
}

fn unify(a: &Option<String>, b: &Option<String>) -> Result<Option<String>, String> {
    match (a, b) {
        (None, x) | (x, None) => Ok(x.clone()),
        (Some(x), Some(y)) if x == y => Ok(Some(x.clone())),
        (Some(x), Some(y)) => Err(format!(
            "a unit mismatch: `{x}` and `{y}` cannot be combined (HYP-11)"
        )),
    }
}

impl Ctx<'_> {
    fn quantity(&mut self, q: &str) -> Result<Option<String>, String> {
        if !self.h.measures().any(|m| m == q) {
            return Err(format!("`{q}` is not a [measures] quantity (HYP-7)"));
        }
        self.uses.quantities.insert(q.to_owned());
        Ok(quantities::get(q).map(|x| x.unit.to_owned()))
    }

    fn num(&mut self, e: &Expr) -> Result<(Option<String>, Level), String> {
        match self.ty(e)? {
            (Ty::Num(u), l) => Ok((u, l)),
            (Ty::Bool, _) => Err(format!("`{e}` is a boolean where a number is needed")),
        }
    }

    fn boolean(&mut self, e: &Expr) -> Result<Level, String> {
        match self.ty(e)? {
            (Ty::Bool, l) => Ok(l),
            (Ty::Num(_), _) => Err(format!("`{e}` is a number where a boolean is needed")),
        }
    }

    fn not_in_guard(&self, what: &str) -> Result<(), String> {
        if self.guard {
            return Err(format!(
                "the guard reads counters and numbers only, not {what} (HYP-10)"
            ));
        }
        Ok(())
    }

    /// A built-in argument that must be a quantity reference: a bare `qname`.
    fn qref(&mut self, f: &str, a: Option<&Arg>) -> Result<Option<String>, String> {
        match a {
            Some(Arg::Expr(Expr::Quantity(q))) => self.quantity(q),
            _ => Err(format!("`{f}` takes a quantity name first (HYP-13)")),
        }
    }

    fn ci(f: &str, a: Option<&Arg>) -> Result<(), String> {
        match a {
            None => Ok(()),
            Some(Arg::Ci(c)) if *c > 0.0 && *c < 1.0 => Ok(()),
            Some(Arg::Ci(c)) => Err(format!("`ci = {c}` must lie strictly between 0 and 1")),
            Some(_) => Err(format!("`{f}`'s last argument is `ci = <level>`")),
        }
    }

    fn call(&mut self, f: &str, args: &[Arg]) -> Result<(Ty, Level), String> {
        let arity = |n: std::ops::RangeInclusive<usize>| -> Result<(), String> {
            if n.contains(&args.len()) {
                Ok(())
            } else {
                Err(format!(
                    "`{f}` takes {} to {} arguments",
                    n.start(),
                    n.end()
                ))
            }
        };
        let expr = |i: usize| -> Result<&Expr, String> {
            match args.get(i) {
                Some(Arg::Expr(e)) => Ok(e),
                _ => Err(format!("`{f}`'s argument {} is an expression", i + 1)),
            }
        };
        match f {
            "abs" => {
                arity(1..=1)?;
                let (u, l) = self.num(expr(0)?)?;
                Ok((Ty::Num(u), l))
            }
            "min" | "max" => {
                arity(2..=2)?;
                let (a, la) = self.num(expr(0)?)?;
                let (b, lb) = self.num(expr(1)?)?;
                Ok((Ty::Num(unify(&a, &b)?), la.max(lb)))
            }
            "effect" | "rel_effect" => {
                arity(1..=1)?;
                let u = self.qref(f, args.first())?;
                self.uses.free_terms += 1;
                Ok((
                    Ty::Num(if f == "effect" { u } else { None }),
                    Level::PerCell,
                ))
            }
            "ci_low" | "ci_high" => {
                arity(1..=2)?;
                let u = self.qref(f, args.first())?;
                Self::ci(f, args.get(1))?;
                self.uses.free_terms += 1;
                self.uses.interval_or_floor = true;
                Ok((Ty::Num(u), Level::PerCell))
            }
            "max_over_knobs" | "min_over_knobs" => {
                arity(1..=1)?;
                let (u, _) = self.num(expr(0)?)?;
                self.uses.aggregated = true;
                Ok((Ty::Num(u), Level::Slice))
            }
            "noise_floor" => {
                arity(2..=3)?;
                let u = self.qref(f, args.first())?;
                if args.get(1) != Some(&Arg::Control) {
                    return Err("`noise_floor` takes `control` second (HYP-13)".into());
                }
                Self::ci(f, args.get(2))?;
                self.uses.interval_or_floor = true;
                Ok((Ty::Num(u), Level::Slice))
            }
            _ => Err(format!("`{f}` is not a built-in (HYP-13)")),
        }
    }

    fn select(&mut self, q: &str, sels: &[Selector]) -> Result<(Ty, Level), String> {
        let unit = self.quantity(q)?;
        let mut arm: Option<bool> = None;
        let mut fixed: BTreeMap<String, Lit> = BTreeMap::new();
        for s in sels {
            let (param, value) = match s {
                Selector::Control | Selector::Treatment => {
                    let control = *s == Selector::Control;
                    if arm.is_some() {
                        return Err(format!("`{q}(…)` names an arm twice (HYP-12)"));
                    }
                    arm = Some(control);
                    continue;
                }
                Selector::Fix(p, v) => (p.clone(), v.clone()),
                Selector::Value(v) => match self.h.param_of_value(v) {
                    Some(p) => (p.name.clone(), Lit::Ident(v.clone())),
                    None => {
                        return Err(format!(
                            "`{v}` is not an enum value of any parameter; a bool or number is written `pname = value` (HYP-12)"
                        ));
                    }
                },
            };
            let Some(p) = self.h.params.get(&param) else {
                return Err(format!("`{param}` is not a [varies] parameter"));
            };
            if !p.domain.admits(&value) {
                return Err(format!(
                    "`{param} = {value}` lies outside its domain (HYP-12)"
                ));
            }
            if !p.pooled {
                return Err(format!(
                    "`{param}` is not pooled: its verdicts are per value, so a selector cannot fix it (HYP-12)"
                ));
            }
            if fixed.insert(param.clone(), value).is_some() {
                return Err(format!("`{q}(…)` fixes `{param}` twice (HYP-12)"));
            }
        }
        if arm == Some(true) {
            self.uses.control_select = true;
        } else {
            self.uses.treatment_fixes.push(fixed.clone());
        }
        let pooled = self.h.params.values().filter(|p| p.pooled).count();
        let level = if fixed.len() == pooled {
            Level::Slice
        } else {
            Level::PerCell
        };
        Ok((Ty::Num(unit), level))
    }

    fn ty(&mut self, e: &Expr) -> Result<(Ty, Level), String> {
        Ok(match e {
            Expr::Num(_) => (Ty::Num(None), Level::Slice),
            Expr::Counter(c) => {
                if !self.guard && *c == Counter::ProvidersReported {
                    return Err("a predicate may not read `providers_reported` (HYP-12)".into());
                }
                if self.guard && *c == Counter::ProvidersReported && !self.h.has_provider() {
                    return Err(
                        "the guard reads `providers_reported`, but the file has no `provider` parameter (HYP-10)"
                            .into(),
                    );
                }
                (Ty::Num(None), Level::Slice)
            }
            Expr::Quantity(q) => {
                self.not_in_guard("a quantity")?;
                let u = self.quantity(q)?;
                self.uses.free_terms += 1;
                self.uses.treatment_fixes.push(BTreeMap::new());
                (Ty::Num(u), Level::PerCell)
            }
            Expr::Select(q, sels) => {
                self.not_in_guard("a select")?;
                self.select(q, sels)?
            }
            Expr::Call(f, args) => {
                self.not_in_guard("a built-in")?;
                self.call(f, args)?
            }
            Expr::Neg(x) => {
                self.not_in_guard("arithmetic")?;
                let (u, l) = self.num(x)?;
                (Ty::Num(u), l)
            }
            Expr::Arith(op, a, b) => {
                self.not_in_guard("arithmetic")?;
                let (ua, la) = self.num(a)?;
                let (ub, lb) = self.num(b)?;
                let u = match op {
                    crate::predicate::ArithOp::Add | crate::predicate::ArithOp::Sub => {
                        unify(&ua, &ub)?
                    }
                    _ => None,
                };
                (Ty::Num(u), la.max(lb))
            }
            Expr::Cmp(_, a, b) => {
                let (ua, la) = self.num(a)?;
                let (ub, lb) = self.num(b)?;
                unify(&ua, &ub)?;
                (Ty::Bool, la.max(lb))
            }
            Expr::And(a, b) | Expr::Or(a, b) => {
                let la = self.boolean(a)?;
                let lb = self.boolean(b)?;
                (Ty::Bool, la.max(lb))
            }
            Expr::Not(x) => (Ty::Bool, self.boolean(x)?),
            Expr::At(inner, all, range) => {
                self.not_in_guard("an `at` clause")?;
                self.boolean(inner)?;
                if let AtRange::Bound(p, _, _) = range {
                    match self.h.params.get(p) {
                        None => return Err(format!("`at … {p}`: `{p}` is not a parameter")),
                        Some(param) if !param.domain.is_numeric() => {
                            return Err(format!(
                                "`at … {p}` bounds a parameter that is not a range; use `at … cells`"
                            ));
                        }
                        Some(param) if !param.pooled => {
                            return Err(format!("`at … {p}` ranges over a non-pooled parameter"));
                        }
                        Some(_) => {}
                    }
                }
                self.uses.at = Some((*all, range.clone()));
                (Ty::Bool, Level::Slice)
            }
        })
    }
}

/// The number of cells in a slice: the product of the pooled parameters' grid
/// sizes, `None` when one of them is unbounded (HYP-14).
#[must_use]
pub fn cells_per_slice(h: &Hypothesis) -> Option<usize> {
    h.params
        .values()
        .filter(|p| p.pooled)
        .try_fold(1usize, |n, p| p.domain.size().map(|s| n.saturating_mul(s)))
}

/// Walk the predicate: its uses, and its level.
pub fn uses(h: &Hypothesis) -> Result<(Uses, Level), String> {
    let mut cx = Ctx {
        h,
        guard: false,
        uses: Uses::default(),
    };
    let (ty, level) = cx.ty(&h.predicate)?;
    if ty != Ty::Bool {
        return Err("the predicate is a number; a falsifier is a boolean (HYP-10)".into());
    }
    Ok((cx.uses, level))
}

/// Check a parsed file's predicates; on success the file, with any warnings.
pub fn check(mut h: Hypothesis) -> Result<Hypothesis, HypError> {
    let fail = |key: &str, m: String| HypError::new(&h.path, Some(key.to_owned()), m);
    let (uses, level) = uses(&h).map_err(|m| fail("falsifier.predicate", m))?;

    // HYP-12: parameters fixed anywhere are fixed in every treatment-arm term,
    // and then no bare quantity or per-cell built-in may leave them free.
    let fixed_anywhere: BTreeSet<&String> = uses
        .treatment_fixes
        .iter()
        .flat_map(BTreeMap::keys)
        .collect();
    if !fixed_anywhere.is_empty() {
        if uses.free_terms > 0 {
            return Err(fail(
                "falsifier.predicate",
                "a predicate that fixes a parameter cannot also use a bare quantity or a per-cell built-in, which would leave it free (HYP-12)".into(),
            ));
        }
        for p in &fixed_anywhere {
            if uses.treatment_fixes.iter().any(|f| !f.contains_key(*p)) {
                return Err(fail(
                    "falsifier.predicate",
                    format!("`{p}` is fixed in one treatment term and free in another (HYP-12)"),
                ));
            }
        }
    }
    if uses.control_select {
        for p in &fixed_anywhere {
            let values: BTreeSet<String> = uses
                .treatment_fixes
                .iter()
                .filter_map(|f| f.get(*p).map(ToString::to_string))
                .collect();
            if values.len() > 1 {
                return Err(fail(
                    "falsifier.predicate",
                    format!(
                        "with a control term, `{p}` may be fixed to one value only, not {values:?} (HYP-12)"
                    ),
                ));
            }
        }
    }
    // HYP-14: a per-cell predicate over more than one cell needs `at` or an aggregate.
    if level == Level::PerCell && cells_per_slice(&h) != Some(1) {
        return Err(fail(
            "falsifier.predicate",
            "a per-cell predicate over a slice of more than one cell is ambiguous: add `at all …`, `at any …` or an aggregate (HYP-14)".into(),
        ));
    }
    // HYP-9: a twin needs a tolerance for every quantity the predicate reads.
    if h.design.twin_required {
        for q in &uses.quantities {
            if !h.design.sim_live_tolerance.contains_key(q) {
                return Err(fail(
                    "design.sim_live_tolerance",
                    format!(
                        "`twin_required` and the predicate reads `{q}`, which has no tolerance (HYP-9)"
                    ),
                ));
            }
        }
    }
    // HYP-10: the guard.
    if let Some(g) = &h.guard {
        let mut cx = Ctx {
            h: &h,
            guard: true,
            uses: Uses::default(),
        };
        match cx.ty(g) {
            Ok((Ty::Bool, _)) => {}
            Ok(_) => {
                return Err(fail(
                    "falsifier.inconclusive_if",
                    "the guard is a number; it must be a boolean".into(),
                ));
            }
            Err(m) => return Err(fail("falsifier.inconclusive_if", m)),
        }
    }
    if matches!(h.control, crate::file::Control::Missing) && uses.control_select {
        h.warnings
            .push("the predicate reads the control arm, but the file names no control".into());
    }
    Ok(h)
}
