//! HYP-11, HYP-13, HYP-14: a falsifier evaluated over a slice's data — arm means
//! per replicate then over replicates, selects and the control a cell maps to,
//! aggregates, `at` clauses, Kleene connectives over strictly undefined values
//! and the outcome HYP-21 reads from them, the noise floor and interval bounds
//! through the slice, the record HYP-22 and HYP-28 read, and checked slice data.

#![allow(clippy::unwrap_used, clippy::expect_used, clippy::panic)] // CON-19: tests are exempt

mod common;

use std::collections::BTreeSet;

use acn_hyp::Hypothesis;
use acn_hyp::bootstrap::{
    Function, bounds, effect_stats, half_width, split_half_stats, stream, stream_name, verdict_seed,
};
use acn_hyp::file::Domain;
use acn_hyp::predicate::Expr;
use acn_hyp::slice::{
    Arm, Cell, CellData, ControlData, ControlKind, Evaluation, Observed, Outcome, SliceData,
    SliceEval, Value, Values, cell_order, evaluate, key,
};
use common::{BASE, candidate, with_predicate};

const Q: &str = "cached_token_ratio";

fn load(text: &str) -> Hypothesis {
    candidate(text, "t1").0.unwrap()
}

fn arm(values: &[f64]) -> Arm {
    Arm {
        replicates: values
            .iter()
            .map(|v| Some(Values::from([(Q.to_owned(), Some(*v))])))
            .collect(),
    }
}

fn cell(knob: bool, mode: &str) -> Cell {
    [
        ("knob".to_owned(), Value::Bool(knob)),
        ("mode".to_owned(), Value::Enum(mode.to_owned())),
    ]
    .into()
}

type Parts = (Vec<CellData>, Vec<ControlData>);

/// BASE's cells (given out of order: the slice sorts them) and controls: each
/// cell maps to the control of its `mode` (the control config fixes
/// `knob = false`, HYP-8). `t(knob, mode)` and `c(mode)` give each arm's values.
fn parts(t: impl Fn(bool, &str) -> Option<Vec<f64>>, c: impl Fn(&str) -> Vec<f64>) -> Parts {
    let controls = ["fast", "slow"]
        .iter()
        .map(|m| ControlData {
            kind: ControlKind::Config,
            config: cell(false, m),
            arm: arm(&c(m)),
        })
        .collect();
    let mut cells = Vec::new();
    for knob in [true, false] {
        for (i, m) in ["slow", "fast"].iter().enumerate() {
            cells.push(CellData {
                cell: cell(knob, m),
                treatment: t(knob, m).map(|v| arm(&v)),
                control: Some(1 - i),
            });
        }
    }
    (cells, controls)
}

fn slice_of(h: &Hypothesis, key: &str, (cells, controls): Parts) -> SliceData {
    SliceData::new(h, key.to_owned(), cells, controls).unwrap()
}

fn slice(t: impl Fn(bool, &str) -> Option<Vec<f64>>, c: impl Fn(&str) -> Vec<f64>) -> SliceData {
    slice_of(&load(BASE), "", parts(t, c))
}

fn run(p: &str, s: &SliceData) -> SliceEval {
    let h = load(&with_predicate(p));
    evaluate(h.predicate(), s, verdict_seed(&h.hash()).unwrap()).unwrap()
}

fn eval(p: &str, s: &SliceData) -> Option<bool> {
    run(p, s).value
}

fn outcome(p: &str, s: &SliceData) -> Outcome {
    run(p, s).outcome
}

fn flat(v: f64) -> Vec<f64> {
    vec![v; 4]
}

/// `n` distinct values: a scrambled sequence scaled into `[offset, offset + 1)`.
fn spread(n: usize, mult: usize, offset: f64) -> Vec<f64> {
    (0..n)
        .map(|i| offset + ((i * mult) % 97) as f64 / 97.0)
        .collect()
}

/// A slice whose controls vary (a nonzero floor) and whose treatment has effect
/// `e(knob, mode)` over its control.
fn noisy(e: impl Fn(bool, &str) -> f64) -> SliceData {
    let base = [0.40, 0.46, 0.41, 0.47];
    slice(
        |k, m| Some(base.iter().map(|b| b + e(k, m)).collect()),
        |_| base.to_vec(),
    )
}

fn lhs(h: &Hypothesis) -> &Expr {
    let Expr::Cmp(_, lhs, _) = h.predicate() else {
        panic!("a comparison")
    };
    lhs
}

/// Cites: HYP-11, HYP-12
#[test]
fn an_arm_is_the_mean_of_its_replicates_and_a_select_reads_one_cell_and_its_control() {
    let s = slice(
        |k, m| {
            Some(match (k, m) {
                (true, "fast") => vec![0.5, 0.7, 0.6, 0.6],
                (true, "slow") => flat(0.95),
                _ => flat(0.1),
            })
        },
        |m| if m == "fast" { flat(0.2) } else { flat(0.9) },
    );
    // treatment (knob = true, fast) = 0.6; its control is the `fast` control, 0.2.
    let p = |op: &str, x: &str| {
        format!("cached_token_ratio(knob = true, fast) - cached_token_ratio(control) {op} {x}")
    };
    assert_eq!(eval(&p(">", "0.39"), &s), Some(true));
    assert_eq!(eval(&p("<", "0.41"), &s), Some(true));
    assert_eq!(eval(&p(">", "0.41"), &s), Some(false));
    // The control a fixed cell maps to: (true, slow) reads the `slow` control,
    // 0.95 − 0.9; the `fast` control would give 0.75.
    let slow = "cached_token_ratio(knob = true, slow) - cached_token_ratio(control) < 0.1";
    assert_eq!(eval(slow, &s), Some(true));
    assert_eq!(
        eval("max_over_knobs(effect(cached_token_ratio)) > 0.39", &s),
        Some(true)
    );
    assert_eq!(
        eval("max_over_knobs(effect(cached_token_ratio)) > 0.41", &s),
        Some(false)
    );
    // min over the four cells: (false, slow) has 0.1 − 0.9.
    assert_eq!(
        eval("min_over_knobs(effect(cached_token_ratio)) < -0.79", &s),
        Some(true)
    );
    assert_eq!(
        eval("min_over_knobs(effect(cached_token_ratio)) < -0.81", &s),
        Some(false)
    );
}

/// Cites: HYP-13
#[test]
fn rel_effect_divides_the_effect_by_the_control() {
    let s = slice(
        |k, _| Some(if k { flat(0.3) } else { flat(0.2) }),
        |_| flat(0.2),
    );
    // (0.3 − 0.2) / 0.2 = 0.5; dividing by the treatment would give 0.33.
    assert_eq!(
        eval("max_over_knobs(rel_effect(cached_token_ratio)) > 0.49", &s),
        Some(true)
    );
    assert_eq!(
        eval("max_over_knobs(rel_effect(cached_token_ratio)) > 0.51", &s),
        Some(false)
    );
    let zero = slice(|_, _| Some(flat(0.3)), |_| flat(0.0));
    assert_eq!(
        outcome(
            "max_over_knobs(rel_effect(cached_token_ratio)) > 0.5",
            &zero
        ),
        Outcome::Undefined,
        "a zero control: division by zero"
    );
}

/// Cites: HYP-11, HYP-12
#[test]
fn an_incomplete_or_undefined_replicate_makes_the_arm_undefined_and_is_never_skipped() {
    let p = "max_over_knobs(effect(cached_token_ratio)) > 10";
    let ok = || parts(|_, _| Some(flat(0.5)), |_| flat(0.5));
    assert_eq!(
        outcome(p, &slice_of(&load(BASE), "", ok())),
        Outcome::NotRefuted
    );
    let (mut cells, controls) = ok();
    cells[0].treatment.as_mut().unwrap().replicates[2] = None;
    let s = slice_of(&load(BASE), "", (cells, controls));
    assert_eq!(
        outcome(p, &s),
        Outcome::Undefined,
        "a replicate that did not complete"
    );
    assert_eq!(s.min_replicates(), Some(3));
    assert_eq!(eval("replicates < 4", &s), Some(true));
    assert_eq!(eval("replicates < 3", &s), Some(false));
    let (cells, mut controls) = ok();
    controls[0].arm.replicates[1] = Some(Values::from([(Q.to_owned(), None)]));
    let s = slice_of(&load(BASE), "", (cells, controls));
    assert_eq!(
        outcome(p, &s),
        Outcome::Undefined,
        "a replicate whose own value is undefined"
    );
    let (cells, mut controls) = ok();
    controls[0].arm.replicates[1] = Some(Values::new());
    let s = slice_of(&load(BASE), "", (cells, controls));
    assert_eq!(
        outcome(p, &s),
        Outcome::Undefined,
        "a quantity the replicate lacks"
    );
    // A missing control counts as no completed replicates.
    let (mut cells, controls) = ok();
    cells[1].control = None;
    let s = slice_of(&load(BASE), "", (cells, controls));
    assert_eq!(s.min_replicates(), Some(0));
}

/// Cites: HYP-11, HYP-14, HYP-21
#[test]
fn a_true_falsifier_with_a_missing_cell_refutes_and_a_false_one_never_passes() {
    // (true, slow) has no runs: a grid cell with no runs is undefined.
    let missing = |big: bool| {
        slice(
            move |k, m| match (k, m) {
                (true, "slow") => None,
                (true, "fast") if big => Some(flat(0.9)),
                _ => Some(flat(0.5)),
            },
            |_| flat(0.5),
        )
    };
    let any = "effect(cached_token_ratio) > 0.3 at any cells";
    let all = "effect(cached_token_ratio) < 0.3 at all cells";
    assert_eq!(
        outcome(any, &missing(true)),
        Outcome::Refuted,
        "true somewhere refutes"
    );
    assert_eq!(
        outcome(any, &missing(false)),
        Outcome::Undefined,
        "false where defined"
    );
    // `at all` is false at the first false cell, but the missing cell is still
    // visited: the value is false and the outcome is not a pass.
    let r = run(all, &missing(true));
    assert_eq!((r.value, r.outcome), (Some(false), Outcome::Undefined));
    assert_eq!(
        outcome(all, &missing(false)),
        Outcome::Undefined,
        "true where defined"
    );
    let mx = "max_over_knobs(effect(cached_token_ratio)) > 0.3";
    assert_eq!(outcome(mx, &missing(true)), Outcome::Undefined);
    // The connectives over an undefined operand.
    let s = missing(false);
    assert_eq!(
        outcome(&format!("{mx} or replicates < 5"), &s),
        Outcome::Refuted,
        "true or U is true"
    );
    let r = run(&format!("{mx} and replicates > 5"), &s);
    assert_eq!(
        (r.value, r.outcome),
        (Some(false), Outcome::Undefined),
        "false and U is false, and not a pass"
    );
    assert_eq!(
        outcome(&format!("not {mx}"), &s),
        Outcome::Undefined,
        "not U is U"
    );
    // With every cell present, a false falsifier passes.
    let full = slice(|_, _| Some(flat(0.5)), |_| flat(0.5));
    assert_eq!(outcome(any, &full), Outcome::NotRefuted);
    assert_eq!(
        outcome(&format!("{mx} and replicates > 5"), &full),
        Outcome::NotRefuted
    );
}

/// Cites: HYP-13
#[test]
fn a_zero_noise_floor_is_undefined_and_a_nonzero_one_decides() {
    let p4 = "max_over_knobs(abs(effect(cached_token_ratio))) < noise_floor(cached_token_ratio, control)";
    let flat_controls = slice(|_, _| Some(flat(0.5)), |_| flat(0.5));
    assert_eq!(
        outcome(p4, &flat_controls),
        Outcome::Undefined,
        "no variation observed"
    );
    assert_eq!(
        outcome(p4, &noisy(|_, _| 0.0)),
        Outcome::Refuted,
        "no effect is below the floor"
    );
    assert_eq!(
        outcome(p4, &noisy(|k, m| if k && m == "slow" { 0.5 } else { 0.0 })),
        Outcome::NotRefuted
    );
}

fn with16(text: &str) -> String {
    text.replace("replicates = 4", "replicates = 16")
}

/// Cites: HYP-13, HYP-15
#[test]
fn the_noise_floor_is_the_largest_split_half_width_over_the_slices_controls() {
    let h = load(&with16(BASE));
    let wide = spread(16, 7, 0.0);
    let narrow: Vec<f64> = spread(16, 11, 0.0).iter().map(|v| 0.4 + v / 50.0).collect();
    let p = load(&with16(&with_predicate(
        "noise_floor(cached_token_ratio, control, ci = 0.9) > 0.5",
    )));
    let seed = verdict_seed(&p.hash()).unwrap();
    let nf = |cfg: &str, v: &[f64]| {
        let name = stream_name("provider=a", cfg, Q, Function::NoiseFloor);
        half_width(
            &split_half_stats(v, stream(seed, &name).unwrap()).unwrap(),
            0.9,
        )
        .unwrap()
    };
    for (fast, slow) in [(&wide, &narrow), (&narrow, &wide)] {
        let s = slice_of(
            &h,
            "provider=a",
            parts(
                |_, _| Some(narrow.clone()),
                |m| {
                    if m == "fast" {
                        fast.clone()
                    } else {
                        slow.clone()
                    }
                },
            ),
        );
        let expected = nf("knob=false,mode=fast", fast).max(nf("knob=false,mode=slow", slow));
        assert!(
            expected > nf("knob=false,mode=fast", &narrow),
            "the wide control decides"
        );
        let ev = Evaluation::new(p.predicate(), &s, seed).unwrap();
        assert_eq!(ev.num(lhs(&p)), Some(expected));
        // Each control's width is recorded.
        let r = ev.run(p.predicate());
        let per = r.readings.keys().filter(|(_, _, k)| k.is_some()).count();
        assert_eq!(per, 2);
    }
    // One undefined replicate in either control leaves the floor undefined.
    let (cells, mut controls) = parts(|_, _| Some(wide.clone()), |_| wide.clone());
    controls[1].arm.replicates[3] = None;
    let s = slice_of(&h, "", (cells, controls));
    assert_eq!(
        outcome("noise_floor(cached_token_ratio, control) > 0", &s),
        Outcome::Undefined
    );
}

/// Cites: HYP-13, HYP-15
#[test]
fn interval_bounds_come_from_the_cells_own_bootstrap_at_the_requested_level() {
    let h = load(&with16(BASE));
    let t = |k: bool, m: &str| {
        let off = match (k, m) {
            (true, "fast") => 0.3,
            (true, "slow") => 0.1,
            _ => 0.0,
        };
        Some(spread(16, 13, 0.2 + off))
    };
    let c = |m: &str| {
        if m == "fast" {
            spread(16, 5, 0.2)
        } else {
            spread(16, 17, 0.25)
        }
    };
    let mut lows = Vec::new();
    for slice_key in ["", "provider=openai"] {
        let s = slice_of(&h, slice_key, parts(t, c));
        for (ci, text) in [(0.95, ""), (0.9, ", ci = 0.9")] {
            let p = load(&with16(&with_predicate(&format!(
                "min_over_knobs(ci_low(cached_token_ratio{text})) > 0"
            ))));
            let seed = verdict_seed(&p.hash()).unwrap();
            let mut expected = f64::INFINITY;
            for cd in s.cells() {
                let mode = cd.cell["mode"].text();
                let name = stream_name(slice_key, &key(&cd.cell), Q, Function::Effect);
                let stats = effect_stats(
                    &cd.treatment.as_ref().unwrap().values(Q).unwrap(),
                    &c(&mode),
                    stream(seed, &name).unwrap(),
                )
                .unwrap();
                expected = expected.min(bounds(&stats, ci).unwrap().0);
            }
            let got = Evaluation::new(p.predicate(), &s, seed)
                .unwrap()
                .num(lhs(&p));
            assert_eq!(got, Some(expected), "slice `{slice_key}`, ci {ci}");
            lows.push(expected);
        }
    }
    assert_ne!(lows[0], lows[1], "the level matters");
    // Both bounds bracket the point effect, and the narrower level lies inside.
    let s = slice_of(&h, "", parts(t, c));
    let at = |p: &str| {
        let h = load(&with16(&with_predicate(&format!("{p} at all cells"))));
        evaluate(h.predicate(), &s, verdict_seed(&h.hash()).unwrap())
            .unwrap()
            .value
    };
    assert_eq!(
        at("ci_low(cached_token_ratio) <= effect(cached_token_ratio)"),
        Some(true)
    );
    assert_eq!(
        at("ci_high(cached_token_ratio) >= effect(cached_token_ratio)"),
        Some(true)
    );
    assert_eq!(
        at("ci_low(cached_token_ratio, ci = 0.9) > ci_low(cached_token_ratio, ci = 0.99)"),
        Some(true)
    );
    assert_eq!(
        at("ci_high(cached_token_ratio, ci = 0.9) < ci_high(cached_token_ratio, ci = 0.99)"),
        Some(true)
    );
    // A level so small that the bounds would cross is refused at load.
    let e = common::err(
        candidate(
            &with_predicate("min_over_knobs(ci_low(cached_token_ratio, ci = 0.0001)) > 0"),
            "t1",
        )
        .0,
    );
    assert!(e.contains("too small"), "{e}");
}

fn with_predicate_in(text: &str, p: &str) -> String {
    text.replace(
        "predicate = \"max_over_knobs(abs(effect(cached_token_ratio))) < noise_floor(cached_token_ratio, control)\"",
        &format!("predicate = {}", common::toml_string(p)),
    )
}

/// Cites: HYP-14
#[test]
fn at_bounds_range_over_the_cells_whose_value_satisfies_them() {
    let text = BASE
        .replace(
            "knob = { kind = \"bool\" }\nmode = { kind = \"enum\", values = [\"fast\", \"slow\"] }",
            "rtt = { kind = \"int_range\", min = 0, max = 300, levels = [50, 150, 300] }\nloss = { kind = \"range\", min = 0, max = 1, levels = [0.0, 0.5] }",
        )
        .replace("config = { knob = false }", "config = { rtt = 50, loss = 0.0 }");
    let h = load(&with_predicate_in(
        &text,
        "effect(cached_token_ratio) < 0.25 at all cells",
    ));
    // Effect 0.1, 0.2, 0.4 by rtt, plus 0.05 with loss.
    let effect = |r: i64, l: f64| {
        (match r {
            50 => 0.1,
            150 => 0.2,
            _ => 0.4,
        }) + l / 10.0
    };
    let c = |r: i64, l: f64| -> Cell {
        [
            ("rtt".to_owned(), Value::Int(r)),
            ("loss".to_owned(), Value::float(l).unwrap()),
        ]
        .into()
    };
    let mut cells = Vec::new();
    for r in [300, 50, 150] {
        for l in [0.5, 0.0] {
            cells.push(CellData {
                cell: c(r, l),
                treatment: Some(arm(&flat(0.5 + effect(r, l)))),
                control: Some(0),
            });
        }
    }
    let control = ControlData {
        kind: ControlKind::Config,
        config: c(50, 0.0),
        arm: arm(&flat(0.5)),
    };
    let s = SliceData::new(&h, String::new(), cells, vec![control]).unwrap();
    let keys: Vec<String> = s.cells().iter().map(|c| key(&c.cell)).collect();
    assert_eq!(
        keys,
        [
            "loss=0.0,rtt=50",
            "loss=0.0,rtt=150",
            "loss=0.0,rtt=300",
            "loss=0.5,rtt=50",
            "loss=0.5,rtt=150",
            "loss=0.5,rtt=300"
        ],
        "numbers in numeric order, not text order"
    );
    let ev = |p: &str| {
        let h = load(&with_predicate_in(&text, p));
        evaluate(h.predicate(), &s, verdict_seed(&h.hash()).unwrap()).unwrap()
    };
    assert_eq!(
        ev("effect(cached_token_ratio) < 0.26 at all rtt <= 150").outcome,
        Outcome::Refuted
    );
    assert_eq!(
        ev("effect(cached_token_ratio) < 0.26 at all rtt <= 300").outcome,
        Outcome::NotRefuted
    );
    assert_eq!(
        ev("effect(cached_token_ratio) > 0.35 at any rtt > 150").outcome,
        Outcome::Refuted
    );
    assert_eq!(
        ev("effect(cached_token_ratio) > 0.35 at any rtt < 300").outcome,
        Outcome::NotRefuted
    );
    // A bound over float values: loss 0.5 has 0.15, 0.25, 0.45; loss 0 has
    // 0.1, 0.2, 0.4.
    assert_eq!(
        ev("effect(cached_token_ratio) < 0.44 at all loss >= 0.5").outcome,
        Outcome::NotRefuted
    );
    assert_eq!(
        ev("effect(cached_token_ratio) < 0.46 at all loss >= 0.5").outcome,
        Outcome::Refuted
    );
    assert_eq!(
        ev("effect(cached_token_ratio) < 0.41 at all loss <= 0").outcome,
        Outcome::Refuted
    );
}

/// Cites: HYP-14, HYP-22
#[test]
fn decision_cells_are_where_an_aggregate_attains_its_value_and_what_decides_at() {
    // Cells in order: 0 (false, fast), 1 (false, slow), 2 (true, fast), 3 (true, slow).
    let s = slice(
        |k, m| {
            Some(if k && m == "slow" {
                flat(0.9)
            } else {
                flat(0.5)
            })
        },
        |_| flat(0.5),
    );
    let d = |p: &str| run(p, &s).decision_cells;
    assert_eq!(
        d("max_over_knobs(effect(cached_token_ratio)) > 0.3"),
        BTreeSet::from([3]),
        "the argmax"
    );
    assert_eq!(
        d("min_over_knobs(effect(cached_token_ratio)) > 0.3"),
        BTreeSet::from([0, 1, 2]),
        "every cell attaining the min"
    );
    assert_eq!(
        d("effect(cached_token_ratio) < 0.3 at all cells"),
        BTreeSet::from([3]),
        "`at all` failing: the first false cell"
    );
    assert_eq!(
        d("effect(cached_token_ratio) < 0.5 at all cells"),
        BTreeSet::from([0, 1, 2, 3]),
        "`at all` holding: every cell"
    );
    assert_eq!(
        d("effect(cached_token_ratio) > 0.3 at any cells"),
        BTreeSet::from([3]),
        "`at any` holding: the first true cell"
    );
    assert_eq!(
        d("cached_token_ratio(knob = true, fast) - cached_token_ratio(control) < 0.1"),
        BTreeSet::from([2]),
        "a slice-level select: the cell it reads"
    );
}

/// Cites: HYP-11, HYP-15
#[test]
fn the_record_holds_every_sub_expression_and_term_and_repeats_bit_for_bit() {
    let s = noisy(|k, m| if k && m == "fast" { 0.03 } else { 0.0 });
    let p4 = "max_over_knobs(abs(effect(cached_token_ratio))) < noise_floor(cached_token_ratio, control)";
    let a = run(p4, &s);
    assert_eq!(a, run(p4, &s), "the same data, the same record");
    let top = a
        .values
        .iter()
        .find(|((e, _), _)| e.starts_with("(max_over_knobs"))
        .unwrap();
    assert_eq!(*top.1, Observed::Bool(a.value));
    // One effect per cell, each at its own context.
    let effects = a
        .values
        .keys()
        .filter(|(e, _)| e == "effect(cached_token_ratio)")
        .count();
    assert_eq!(effects, 4);
    let arms: Vec<_> = a
        .readings
        .iter()
        .filter(|((t, _, _), _)| t.starts_with("cached_token_ratio:treatment"))
        .collect();
    assert_eq!(arms.len(), 4);
    assert!(
        arms.iter()
            .all(|(_, r)| r.completed == Some(4) && r.cell.is_some())
    );
    let s = slice(|_, _| Some(flat(0.5)), |_| flat(0.5));
    let r = run("min_over_knobs(ci_low(cached_token_ratio)) > -1", &s);
    let (_, reading) = r
        .readings
        .iter()
        .find(|((t, _, _), _)| t.contains("ci_low"))
        .unwrap();
    assert_eq!(reading.interval, Some((0.0, 0.0)));
}

/// Cites: HYP-14, HYP-15
#[test]
fn cells_are_ordered_by_name_then_value_and_keyed_in_text_form() {
    // Declaration order, not alphabetical: `slow` before `fast`.
    let h = load(&BASE.replace(
        "values = [\"fast\", \"slow\"]",
        "values = [\"slow\", \"fast\"]",
    ));
    let mut cells = [
        cell(true, "fast"),
        cell(false, "fast"),
        cell(true, "slow"),
        cell(false, "slow"),
    ];
    cells.sort_by(|a, b| cell_order(&h, a, b));
    let keys: Vec<String> = cells.iter().map(key).collect();
    assert_eq!(
        keys,
        [
            "knob=false,mode=slow",
            "knob=false,mode=fast",
            "knob=true,mode=slow",
            "knob=true,mode=fast"
        ]
    );
    let c: Cell = [
        ("rtt".to_owned(), Value::float(150.0).unwrap()),
        ("loss".to_owned(), Value::float(-0.0).unwrap()),
        ("n".to_owned(), Value::Int(-3)),
    ]
    .into();
    assert_eq!(
        key(&c),
        "loss=0.0,n=-3,rtt=150.0",
        "CON-27(c): a range value is a float, −0 is 0"
    );
    assert!(Value::float(f64::NAN).is_none() && Value::float(f64::INFINITY).is_none());
}

/// Cites: HYP-12, HYP-6
#[test]
fn values_parse_from_manifest_text_and_match_selectors_exactly() {
    let int = Domain::IntRange {
        min: 0,
        max: i64::MAX,
        levels: None,
    };
    let range = Domain::Range {
        min: 0.0,
        max: 1.0,
        levels: None,
    };
    let en = Domain::Enum(vec!["fast".into(), "slow".into()]);
    assert_eq!(Value::parse(&int, "150"), Some(Value::Int(150)));
    assert_eq!(Value::parse(&int, "-1"), None, "outside the domain");
    assert_eq!(Value::parse(&int, "150.0"), None);
    assert_eq!(Value::parse(&range, "0.05"), Value::float(0.05));
    assert_eq!(
        Value::parse(&range, "1"),
        None,
        "a range value is written as a float"
    );
    assert_eq!(Value::parse(&en, "slow"), Some(Value::Enum("slow".into())));
    assert_eq!(Value::parse(&en, "medium"), None);
    assert_eq!(Value::parse(&Domain::Bool, "true"), Some(Value::Bool(true)));
    assert_eq!(Value::parse(&Domain::Bool, "1"), None);
    assert!(Value::Int(150).matches("150") && !Value::Int(150).matches("50"));
    assert!(
        !Value::Int(9_007_199_254_740_993).matches("9007199254740992"),
        "integers never compare through a double"
    );
    let f = Value::float(0.05).unwrap();
    assert!(f.matches("0.05") && !f.matches("0.5"));
    assert!(
        Value::Enum("slow".into()).matches("slow") && !Value::Enum("slow".into()).matches("fast")
    );
    assert!(Value::Bool(false).matches("false") && !Value::Bool(false).matches("true"));
}

/// Cites: HYP-11, HYP-21
#[test]
fn slice_data_is_checked_against_the_hypothesis() {
    let h = load(BASE);
    let err = |(cells, controls): Parts| {
        SliceData::new(&h, String::new(), cells, controls)
            .unwrap_err()
            .to_string()
    };
    let ok = || parts(|_, _| Some(flat(0.5)), |_| flat(0.5));
    let (mut cells, controls) = ok();
    cells[0].treatment.as_mut().unwrap().replicates.push(None);
    assert!(err((cells, controls)).contains("not the design's 4"));
    let (mut cells, controls) = ok();
    cells[1].cell = cells[0].cell.clone();
    assert!(err((cells, controls)).contains("twice"));
    let (mut cells, controls) = ok();
    cells[0].cell.remove("mode");
    assert!(err((cells, controls)).contains("every [varies] parameter"));
    let (mut cells, controls) = ok();
    cells[0]
        .cell
        .insert("mode".into(), Value::Enum("medium".into()));
    assert!(err((cells, controls)).contains("not in its domain"));
    let (mut cells, controls) = ok();
    cells[0].cell.insert("knob".into(), Value::Int(1));
    assert!(err((cells, controls)).contains("not in its domain"));
    let (mut cells, controls) = ok();
    cells[0].control = Some(7);
    assert!(err((cells, controls)).contains("does not exist"));
    let (cells, mut controls) = ok();
    controls[1].config = controls[0].config.clone();
    assert!(err((cells, controls)).contains("twice"));
    // An empty slice evaluates to undefined.
    let s = SliceData::new(&h, String::new(), Vec::new(), Vec::new()).unwrap();
    assert_eq!(
        outcome("max_over_knobs(effect(cached_token_ratio)) > 0", &s),
        Outcome::Undefined
    );
    assert_eq!(
        outcome("effect(cached_token_ratio) > 0 at any cells", &s),
        Outcome::Undefined
    );
    assert_eq!(s.min_replicates(), None);
}
