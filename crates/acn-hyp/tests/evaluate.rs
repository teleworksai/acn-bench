//! HYP-11, HYP-13, HYP-14: a falsifier evaluated over a slice's data — arm means
//! per replicate then over replicates, selects and the control a cell maps to,
//! aggregates, `at` clauses, Kleene connectives over strictly undefined values, a
//! zero noise floor, an incomplete arm and a missing grid cell.

#![allow(clippy::unwrap_used, clippy::expect_used, clippy::panic)] // CON-19: tests are exempt

mod common;

use acn_hyp::Hypothesis;
use acn_hyp::bootstrap::verdict_seed;
use acn_hyp::slice::{
    Arm, CellData, ControlData, SliceData, Value, Values, cell_order, evaluate, key,
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

fn cell(knob: bool, mode: &str) -> acn_hyp::slice::Cell {
    [
        ("knob".to_owned(), Value::Bool(knob)),
        ("mode".to_owned(), Value::Enum(mode.to_owned())),
    ]
    .into()
}

/// BASE's slice: four cells, each mapped to the control of its `mode` (the
/// control config fixes `knob = false`, HYP-8). `t(knob, mode)` and `c(mode)`
/// give the four replicate values of each arm.
fn slice(t: impl Fn(bool, &str) -> Option<Vec<f64>>, c: impl Fn(&str) -> Vec<f64>) -> SliceData {
    let controls = ["fast", "slow"]
        .iter()
        .map(|m| ControlData {
            config: cell(false, m),
            arm: arm(&c(m)),
        })
        .collect();
    let mut cells = Vec::new();
    for knob in [false, true] {
        for (i, m) in ["fast", "slow"].iter().enumerate() {
            cells.push(CellData {
                cell: cell(knob, m),
                treatment: t(knob, m).map(|v| arm(&v)),
                control: Some(i),
            });
        }
    }
    SliceData {
        key: String::new(),
        replicates: 4,
        cells,
        controls,
    }
}

fn eval(p: &str, s: &SliceData) -> Option<bool> {
    let h = load(&with_predicate(p));
    evaluate(h.predicate(), s, verdict_seed(&h.hash()).unwrap())
}

fn flat(v: f64) -> Vec<f64> {
    vec![v; 4]
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

/// Cites: HYP-11, HYP-12
#[test]
fn an_arm_is_the_mean_of_its_replicates_and_a_select_reads_one_cell_and_its_control() {
    let s = slice(
        |k, m| {
            Some(if k && m == "fast" {
                vec![0.5, 0.7, 0.6, 0.6]
            } else {
                flat(0.1)
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
    // The same comparison through `effect` at the one cell `at` selects.
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

/// Cites: HYP-11
#[test]
fn an_incomplete_or_undefined_replicate_makes_the_arm_undefined_and_is_never_skipped() {
    let p = "max_over_knobs(effect(cached_token_ratio)) > 10";
    let mut s = slice(|_, _| Some(flat(0.5)), |_| flat(0.5));
    assert_eq!(eval(p, &s), Some(false));
    s.cells[3].treatment.as_mut().unwrap().replicates[2] = None;
    assert_eq!(eval(p, &s), None, "a replicate that did not complete");
    let mut s = slice(|_, _| Some(flat(0.5)), |_| flat(0.5));
    s.controls[0].arm.replicates[1] = Some(Values::from([(Q.to_owned(), None)]));
    assert_eq!(
        eval(p, &s),
        None,
        "a replicate whose own value is undefined"
    );
    let mut s = slice(|_, _| Some(flat(0.5)), |_| flat(0.5));
    s.controls[0].arm.replicates.pop();
    assert_eq!(eval(p, &s), None, "fewer replicates than the design");
    assert_eq!(s.min_replicates(), Some(3));
}

/// Cites: HYP-11, HYP-14, HYP-21
#[test]
fn kleene_connectives_let_a_true_falsifier_fail_with_a_missing_cell_and_never_pass_one() {
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
        eval(any, &missing(true)),
        Some(true),
        "true somewhere refutes"
    );
    assert_eq!(
        eval(any, &missing(false)),
        None,
        "false where defined does not pass"
    );
    assert_eq!(
        eval(all, &missing(true)),
        Some(false),
        "false somewhere decides `at all`"
    );
    assert_eq!(
        eval(all, &missing(false)),
        None,
        "true where defined is not enough"
    );
    assert_eq!(
        eval(
            "max_over_knobs(effect(cached_token_ratio)) > 0.3",
            &missing(true)
        ),
        None
    );
    // `or` and `and` over an undefined operand.
    let s = missing(false);
    let p = "max_over_knobs(effect(cached_token_ratio)) > 0.3 or replicates < 5";
    assert_eq!(eval(p, &s), Some(true), "true or U is true");
    let p = "max_over_knobs(effect(cached_token_ratio)) > 0.3 and replicates > 5";
    assert_eq!(eval(p, &s), Some(false), "false and U is false");
    let p = "not max_over_knobs(effect(cached_token_ratio)) > 0.3";
    assert_eq!(eval(p, &s), None, "not U is U");
}

/// Cites: HYP-13
#[test]
fn a_zero_noise_floor_is_undefined_and_a_nonzero_one_decides() {
    let p4 = "max_over_knobs(abs(effect(cached_token_ratio))) < noise_floor(cached_token_ratio, control)";
    let flat_controls = slice(|_, _| Some(flat(0.5)), |_| flat(0.5));
    assert_eq!(
        eval(p4, &flat_controls),
        None,
        "no variation observed: nothing to compare with"
    );
    assert_eq!(
        eval(p4, &noisy(|_, _| 0.0)),
        Some(true),
        "no effect is below the floor"
    );
    assert_eq!(
        eval(p4, &noisy(|k, m| if k && m == "slow" { 0.5 } else { 0.0 })),
        Some(false)
    );
}

/// Cites: HYP-13, HYP-15
#[test]
fn interval_bounds_bracket_the_effect_and_narrow_with_the_level() {
    let s2 = slice(
        |k, _| {
            Some(if k {
                vec![0.6, 0.9, 0.7, 0.8]
            } else {
                flat(0.5)
            })
        },
        |_| flat(0.5),
    );
    let at = |p: &str| eval(&format!("{p} at all cells"), &s2);
    // knob = true cells: effect values 0.1..0.4, mean 0.25; knob = false: 0.
    assert_eq!(
        at("ci_low(cached_token_ratio) <= effect(cached_token_ratio)"),
        Some(true)
    );
    assert_eq!(
        at("ci_high(cached_token_ratio) >= effect(cached_token_ratio)"),
        Some(true)
    );
    assert_eq!(
        at("ci_low(cached_token_ratio) <= ci_high(cached_token_ratio)"),
        Some(true)
    );
    assert_eq!(
        at("ci_low(cached_token_ratio, ci = 0.9) >= ci_low(cached_token_ratio, ci = 0.99)"),
        Some(true)
    );
    assert_eq!(
        at("ci_high(cached_token_ratio, ci = 0.9) <= ci_high(cached_token_ratio, ci = 0.99)"),
        Some(true)
    );
    // The bounds stay within the range of the paired differences.
    assert_eq!(at("ci_low(cached_token_ratio) >= 0"), Some(true));
    assert_eq!(at("ci_high(cached_token_ratio) <= 0.4"), Some(true));
    assert_eq!(
        eval(
            "ci_low(cached_token_ratio, ci = 0.5) > 0.1 at any cells",
            &s2
        ),
        Some(true)
    );
    // Paired resampling of a constant shift: every resample is exactly the shift.
    let s3 = noisy(|k, _| if k { 0.2 } else { 0.0 });
    let at3 = |p: &str| eval(&format!("{p} at any cells"), &s3);
    assert_eq!(at3("ci_low(cached_token_ratio) > 0.199999"), Some(true));
    assert_eq!(at3("ci_high(cached_token_ratio) > 0.200001"), Some(false));
}

/// Cites: HYP-14
#[test]
fn at_bounds_range_over_the_cells_whose_value_satisfies_them() {
    let text = BASE
        .replace(
            "knob = { kind = \"bool\" }\nmode = { kind = \"enum\", values = [\"fast\", \"slow\"] }",
            "rtt = { kind = \"int_range\", min = 0, max = 300, levels = [50, 150, 300] }",
        )
        .replace("config = { knob = false }", "config = { rtt = 50 }");
    let h = load(&with_predicate_in(
        &text,
        "effect(cached_token_ratio) < 0.25 at all rtt <= 150",
    ));
    let rtt = |v: i64| -> acn_hyp::slice::Cell { [("rtt".to_owned(), Value::Int(v))].into() };
    // Effect grows with rtt: 0.1, 0.2, 0.4.
    let effect = |r: i64| match r {
        50 => 0.1,
        150 => 0.2,
        _ => 0.4,
    };
    let s = SliceData {
        key: String::new(),
        replicates: 4,
        cells: [50, 150, 300]
            .iter()
            .map(|r| CellData {
                cell: rtt(*r),
                treatment: Some(arm(&flat(0.5 + effect(*r)))),
                control: Some(0),
            })
            .collect(),
        controls: vec![ControlData {
            config: rtt(50),
            arm: arm(&flat(0.5)),
        }],
    };
    let seed = verdict_seed(&h.hash()).unwrap();
    assert_eq!(
        evaluate(h.predicate(), &s, seed),
        Some(true),
        "300 is outside the bound"
    );
    let h = load(&with_predicate_in(
        &text,
        "effect(cached_token_ratio) < 0.25 at all rtt <= 300",
    ));
    assert_eq!(evaluate(h.predicate(), &s, seed), Some(false));
    let h = load(&with_predicate_in(
        &text,
        "effect(cached_token_ratio) > 0.35 at any rtt > 150",
    ));
    assert_eq!(evaluate(h.predicate(), &s, seed), Some(true));
    let h = load(&with_predicate_in(
        &text,
        "effect(cached_token_ratio) > 0.35 at any rtt < 300",
    ));
    assert_eq!(evaluate(h.predicate(), &s, seed), Some(false));
}

fn with_predicate_in(text: &str, p: &str) -> String {
    text.replace(
        "predicate = \"max_over_knobs(abs(effect(cached_token_ratio))) < noise_floor(cached_token_ratio, control)\"",
        &format!("predicate = {}", common::toml_string(p)),
    )
}

/// Cites: HYP-14, HYP-15
#[test]
fn cells_are_ordered_by_name_then_value_and_keyed_in_text_form() {
    let h = load(BASE);
    let mut cells = [
        cell(true, "slow"),
        cell(false, "slow"),
        cell(true, "fast"),
        cell(false, "fast"),
    ];
    cells.sort_by(|a, b| cell_order(&h, a, b));
    let keys: Vec<String> = cells.iter().map(key).collect();
    assert_eq!(
        keys,
        [
            "knob=false,mode=fast",
            "knob=false,mode=slow",
            "knob=true,mode=fast",
            "knob=true,mode=slow"
        ]
    );
    let c: acn_hyp::slice::Cell = [
        ("rtt".to_owned(), Value::float(150.0).unwrap()),
        ("loss".to_owned(), Value::float(0.05).unwrap()),
        ("n".to_owned(), Value::Int(-3)),
    ]
    .into();
    assert_eq!(
        key(&c),
        "loss=0.05,n=-3,rtt=150.0",
        "CON-27(c): a range value is a float"
    );
    assert!(Value::float(f64::NAN).is_none() && Value::float(f64::INFINITY).is_none());
}

/// Cites: HYP-11, HYP-15
#[test]
fn evaluation_is_deterministic_and_reads_one_set_of_resamples_per_interval() {
    let s = noisy(|k, m| if k && m == "fast" { 0.03 } else { 0.0 });
    let p4 = "max_over_knobs(abs(effect(cached_token_ratio))) < noise_floor(cached_token_ratio, control)";
    let a = eval(p4, &s);
    assert!(a.is_some());
    assert_eq!(a, eval(p4, &s));
}
