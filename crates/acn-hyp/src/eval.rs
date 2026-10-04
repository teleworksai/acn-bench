//! The one evaluator of HYP-11 and HYP-13: arithmetic over defined values,
//! undefined propagated strictly, Kleene's three-valued connectives. Where the
//! values come from is a [`Source`]: lint's probe cell (HYP-27) and, from T05.2,
//! the verdict engine's cells. Both read the same rules, so lint's witnesses and
//! real verdicts cannot drift apart (ADR-18).

use std::fmt;

use crate::predicate::{Arg, ArithOp, AtRange, Builtin, Counter, Expr, Lit, Selector};

/// One value a predicate reads, as a [`Source`] supplies it.
#[derive(Debug, Clone, PartialEq, Eq, PartialOrd, Ord)]
pub enum Term {
    /// A quantity in one arm, with the parameters a `select` fixes (sorted).
    Arm {
        quantity: String,
        control: bool,
        fixes: Vec<(String, String)>,
    },
    /// An interval bound of `effect(q)` at a confidence level (as `f64` bits).
    CiLow {
        quantity: String,
        ci: u64,
    },
    CiHigh {
        quantity: String,
        ci: u64,
    },
    NoiseFloor {
        quantity: String,
        ci: u64,
    },
    Counter(CounterKind),
}

/// The counters, as terms.
#[derive(Debug, Clone, Copy, PartialEq, Eq, PartialOrd, Ord)]
pub enum CounterKind {
    Replicates,
    ProvidersReported,
}

impl fmt::Display for Term {
    fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
        let ci = |bits: &u64| f64::from_bits(*bits);
        match self {
            Self::Arm {
                quantity,
                control,
                fixes,
            } => {
                let arm = if *control { "control" } else { "treatment" };
                let fx: Vec<String> = fixes.iter().map(|(p, v)| format!("{p}={v}")).collect();
                write!(f, "{quantity}:{arm}[{}]", fx.join(","))
            }
            Self::CiLow { quantity, ci: c } => write!(f, "{quantity}:ci_low@{}", ci(c)),
            Self::CiHigh { quantity, ci: c } => write!(f, "{quantity}:ci_high@{}", ci(c)),
            Self::NoiseFloor { quantity, ci: c } => write!(f, "{quantity}:noise_floor@{}", ci(c)),
            Self::Counter(CounterKind::Replicates) => f.write_str("replicates"),
            Self::Counter(CounterKind::ProvidersReported) => f.write_str("providers_reported"),
        }
    }
}

/// HYP-13's default confidence level.
pub const DEFAULT_CI: f64 = 0.95;

fn ci_of(args: &[Arg]) -> u64 {
    args.iter()
        .find_map(|a| match a {
            Arg::Ci(c) => Some(*c),
            _ => None,
        })
        .unwrap_or(DEFAULT_CI)
        .to_bits()
}

/// The quantity a built-in's first argument names.
#[must_use]
pub fn qref(args: &[Arg]) -> Option<&str> {
    match args.first() {
        Some(Arg::Expr(Expr::Quantity(q))) => Some(q),
        _ => None,
    }
}

/// The arm term of a bare quantity or a `select` whose selectors are normalised
/// to `pname = value` (as loading leaves them).
#[must_use]
pub fn arm_term(e: &Expr) -> Option<Term> {
    match e {
        Expr::Quantity(q) => Some(Term::Arm {
            quantity: q.clone(),
            control: false,
            fixes: Vec::new(),
        }),
        Expr::Select(q, sels) => {
            let mut fixes: Vec<(String, String)> = sels
                .iter()
                .filter_map(|s| match s {
                    Selector::Fix(p, v) => Some((p.clone(), lit_text(v))),
                    _ => None,
                })
                .collect();
            fixes.sort();
            Some(Term::Arm {
                quantity: q.clone(),
                control: sels.contains(&Selector::Control),
                fixes,
            })
        }
        _ => None,
    }
}

/// A selector value as text, one spelling per value.
#[must_use]
pub fn lit_text(v: &Lit) -> String {
    v.to_string()
}

fn arm(q: &str, control: bool) -> Term {
    Term::Arm {
        quantity: q.to_owned(),
        control,
        fixes: Vec::new(),
    }
}

/// Every term `e` reads, each once, in the order first met.
pub fn terms(e: &Expr, out: &mut Vec<Term>) {
    let mut add = |t: Term| {
        if !out.contains(&t) {
            out.push(t);
        }
    };
    match e {
        Expr::Num(_) => {}
        Expr::Counter(Counter::Replicates) => add(Term::Counter(CounterKind::Replicates)),
        Expr::Counter(Counter::ProvidersReported) => {
            add(Term::Counter(CounterKind::ProvidersReported));
        }
        Expr::Quantity(_) | Expr::Select(..) => {
            if let Some(t) = arm_term(e) {
                add(t);
            }
        }
        Expr::Call(b, args) => {
            let q = qref(args).unwrap_or_default().to_owned();
            let ci = ci_of(args);
            match b {
                Builtin::Effect | Builtin::RelEffect => {
                    add(arm(&q, false));
                    add(arm(&q, true));
                }
                Builtin::CiLow | Builtin::CiHigh => {
                    add(Term::CiLow {
                        quantity: q.clone(),
                        ci,
                    });
                    add(Term::CiHigh { quantity: q, ci });
                }
                Builtin::NoiseFloor => add(Term::NoiseFloor { quantity: q, ci }),
                Builtin::Abs
                | Builtin::Min
                | Builtin::Max
                | Builtin::MaxOverKnobs
                | Builtin::MinOverKnobs => {
                    for a in args {
                        if let Arg::Expr(x) = a {
                            terms(x, out);
                        }
                    }
                }
            }
        }
        Expr::Neg(x) | Expr::Not(x) | Expr::At(x, _, _) => terms(x, out),
        Expr::Arith(_, a, b) | Expr::Cmp(_, a, b) | Expr::And(a, b) | Expr::Or(a, b) => {
            terms(a, out);
            terms(b, out);
        }
    }
}

/// Where an evaluation's values come from.
pub trait Source {
    /// A term's value in the current cell, or `None` when it is undefined.
    fn term(&self, t: &Term) -> Option<f64>;
    /// `max_over_knobs(x)` (`max = true`) or `min_over_knobs(x)` over the slice.
    fn over_knobs(&self, max: bool, x: &Expr) -> Option<f64>;
    /// `inner at all|any range` over the slice.
    fn quantify(&self, all: bool, range: &AtRange, inner: &Expr) -> Option<bool>;
}

fn finite(v: f64) -> Option<f64> {
    v.is_finite().then_some(v)
}

/// A numeric expression's value (HYP-11): `None` is undefined.
pub fn num<S: Source + ?Sized>(e: &Expr, s: &S) -> Option<f64> {
    let v = match e {
        Expr::Num(n) => Some(*n),
        Expr::Counter(Counter::Replicates) => s.term(&Term::Counter(CounterKind::Replicates)),
        Expr::Counter(Counter::ProvidersReported) => {
            s.term(&Term::Counter(CounterKind::ProvidersReported))
        }
        Expr::Quantity(_) | Expr::Select(..) => arm_term(e).and_then(|t| s.term(&t)),
        Expr::Neg(x) => num(x, s).map(|v| -v),
        Expr::Arith(op, a, b) => {
            let (a, b) = (num(a, s)?, num(b, s)?);
            match op {
                ArithOp::Add => Some(a + b),
                ArithOp::Sub => Some(a - b),
                ArithOp::Mul => Some(a * b),
                ArithOp::Div if b == 0.0 => None,
                ArithOp::Div => Some(a / b),
            }
        }
        Expr::Call(b, args) => {
            let arg = |i: usize| match args.get(i) {
                Some(Arg::Expr(x)) => num(x, s),
                _ => None,
            };
            let expr = |i: usize| match args.get(i) {
                Some(Arg::Expr(x)) => Some(x),
                _ => None,
            };
            let q = qref(args).unwrap_or_default();
            let ci = ci_of(args);
            match b {
                Builtin::Abs => arg(0).map(f64::abs),
                Builtin::Min => Some(arg(0)?.min(arg(1)?)),
                Builtin::Max => Some(arg(0)?.max(arg(1)?)),
                Builtin::MaxOverKnobs => s.over_knobs(true, expr(0)?),
                Builtin::MinOverKnobs => s.over_knobs(false, expr(0)?),
                Builtin::Effect => Some(s.term(&arm(q, false))? - s.term(&arm(q, true))?),
                Builtin::RelEffect => {
                    let c = s.term(&arm(q, true))?;
                    if c == 0.0 {
                        None
                    } else {
                        Some((s.term(&arm(q, false))? - c) / c)
                    }
                }
                Builtin::CiLow => s.term(&Term::CiLow {
                    quantity: q.to_owned(),
                    ci,
                }),
                Builtin::CiHigh => s.term(&Term::CiHigh {
                    quantity: q.to_owned(),
                    ci,
                }),
                // HYP-13: a zero noise floor is undefined.
                Builtin::NoiseFloor => s
                    .term(&Term::NoiseFloor {
                        quantity: q.to_owned(),
                        ci,
                    })
                    .filter(|v| *v != 0.0),
            }
        }
        Expr::Cmp(..) | Expr::And(..) | Expr::Or(..) | Expr::Not(_) | Expr::At(..) => None,
    };
    v.and_then(finite)
}

/// A boolean expression's value under Kleene's three-valued logic (HYP-11).
pub fn truth<S: Source + ?Sized>(e: &Expr, s: &S) -> Option<bool> {
    match e {
        Expr::Cmp(op, a, b) => Some(op.apply(num(a, s)?, num(b, s)?)),
        Expr::And(a, b) => match (truth(a, s), truth(b, s)) {
            (Some(false), _) | (_, Some(false)) => Some(false),
            (Some(true), Some(true)) => Some(true),
            _ => None,
        },
        Expr::Or(a, b) => match (truth(a, s), truth(b, s)) {
            (Some(true), _) | (_, Some(true)) => Some(true),
            (Some(false), Some(false)) => Some(false),
            _ => None,
        },
        Expr::Not(x) => truth(x, s).map(|b| !b),
        Expr::At(inner, all, range) => s.quantify(*all, range, inner),
        _ => None,
    }
}
