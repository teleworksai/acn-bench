//! The L2 twin (SPEC 085 LOOP-12, LOOP-16). The cells a twin runs are the
//! decision cells of the loop's final verdict and its *k* best and worst
//! cells, chosen from that verdict alone.

#![allow(clippy::unwrap_used, clippy::expect_used, clippy::panic)] // CON-19: tests are exempt

mod common;

use acn_hyp::Hypothesis;
use acn_hyp::loop_run::Code;
use acn_hyp::loop_twin::{Chosen, Why, choose};
use acn_hyp::read::BundleData;
use acn_hyp::verdict::{Verdict, verdict};
use common::bundles::{Spec, alt, bundle, engine};
use common::exec::{Exec, TWO, args, dir_with, run_loop};

fn base() -> Hypothesis {
    let (h, _dir) = common::candidate(common::BASE, "t1");
    h.unwrap()
}

/// BASE's control and four treatment cells in `sim`: knob=true moves the
/// ratio by 0.3 when fast and 0.35 when slow, knob=false by nothing, so the
/// slow knob=true cell decides `max_over_knobs`. `keep` decides which
/// treatment cells are made, by knob and mode.
fn arms(h: &Hypothesis, keep: &dyn Fn(&str, &str) -> bool) -> Vec<BundleData> {
    let base = alt(4, 0.40, 0.46);
    let shifted = |e: f64| base.iter().map(|x| x.map(|x| x + e)).collect();
    let mut out = Vec::new();
    for m in ["fast", "slow"] {
        out.push(bundle(
            h,
            &Spec::new(
                &format!("c-{m}"),
                &[("knob", "false"), ("mode", m)],
                "control",
                shifted(0.0),
            ),
        ));
        for k in ["false", "true"] {
            if !keep(k, m) {
                continue;
            }
            let e = match (k, m) {
                ("true", "fast") => 0.3,
                ("true", _) => 0.35,
                _ => 0.0,
            };
            out.push(bundle(
                h,
                &Spec::new(
                    &format!("t-{k}-{m}"),
                    &[("knob", k), ("mode", m)],
                    "treatment",
                    shifted(e),
                ),
            ));
        }
    }
    out
}

fn judge(h: &Hypothesis, b: Vec<BundleData>) -> Verdict {
    verdict(h, b, engine()).unwrap()
}

/// A chosen cell as `knob,mode` and its reasons.
fn named(c: &[Chosen]) -> Vec<(String, Vec<Why>)> {
    c.iter()
        .map(|c| {
            let v = |n: &str| c.cell.get(n).map(|x| x.text()).unwrap_or_default();
            (format!("{},{}", v("knob"), v("mode")), c.reasons.clone())
        })
        .collect()
}

/// Cites: LOOP-12
#[test]
fn a_twin_takes_the_decision_cells_and_the_k_best_and_worst_once_each_in_order() {
    let h = base();
    let l1 = judge(&h, arms(&h, &|_, _| true));
    // --top 0: the decision cell alone, the slow knob=true cell.
    let c = choose(&h, &l1, 0).unwrap();
    assert_eq!(named(&c), [("true,slow".to_owned(), vec![Why::Decision])]);
    // --top 1: the best is the decision cell; the worst is the first of the
    // two zero effects in HYP-14 order.
    let c = choose(&h, &l1, 1).unwrap();
    let zero_first = named(&choose(&h, &l1, 4).unwrap())
        .into_iter()
        .find(|(k, _)| k.starts_with("false"))
        .unwrap()
        .0;
    let got = named(&c);
    assert_eq!(got.len(), 2, "{got:?}");
    assert!(got.contains(&("true,slow".to_owned(), vec![Why::Decision, Why::Best])));
    assert!(got.contains(&(zero_first, vec![Why::Worst])));
    // Each cell once, in slice-key and HYP-14 order.
    let order: Vec<usize> = c.iter().map(|c| c.cell_index).collect();
    assert!(order.windows(2).all(|w| w[0] < w[1]), "{order:?}");
    // A k beyond the ranked cells takes them all: every cell is both among
    // the best four and the worst four.
    let all = choose(&h, &l1, 10).unwrap();
    assert_eq!(all.len(), 4);
    for c in &all {
        assert!(c.reasons.contains(&Why::Best) && c.reasons.contains(&Why::Worst));
    }
    assert_eq!(
        all[all
            .iter()
            .position(|c| c.reasons[0] == Why::Decision)
            .unwrap()]
        .reasons,
        [Why::Decision, Why::Best, Why::Worst]
    );
}

/// Cites: LOOP-12
#[test]
fn a_cell_with_no_defined_effect_is_never_ranked_and_an_empty_choice_is_refused() {
    let h = base();
    // Only the fast cells have a treatment: the slow ones have no effect, so
    // they are neither best nor worst.
    let l1 = judge(&h, arms(&h, &|_, m| m == "fast"));
    for c in choose(&h, &l1, 10).unwrap() {
        assert_eq!(c.cell.get("mode").unwrap().text(), "fast", "{c:?}");
    }
    // No control: no effect anywhere, and the falsifier is never consulted,
    // so there is no decision cell either.
    let no_control: Vec<BundleData> = arms(&h, &|_, _| true)
        .into_iter()
        .filter(|b| b.manifest.params.get("arms").map(String::as_str) != Some("control"))
        .collect();
    let l1 = judge(&h, no_control);
    let e = choose(&h, &l1, 1).unwrap_err();
    assert_eq!(e.code, Code::NothingToTwin, "{e}");
}

/// Cites: LOOP-12, LOOP-11
#[test]
fn the_first_best_and_worst_a_twin_ranks_are_the_reports_own() {
    let dir = dir_with(TWO);
    let d = dir.path();
    let mut ex = Exec::new(d);
    let c = run_loop(d, args(10), &mut ex).unwrap();
    let r = common::exec::report(&c);
    let h = acn_hyp::load_in(&d.join("zz.toml"), d).unwrap();
    let bundles: Vec<BundleData> = common::exec::bundles(d)
        .values()
        .map(|p| acn_hyp::read::read(p).unwrap())
        .collect();
    let l1 = verdict(&h, bundles, ex.bin().engine_hash).unwrap();
    assert_eq!(l1.verdict_id, c.verdict_id);
    let chosen = choose(&h, &l1, 1).unwrap();
    for (why, field) in [(Why::Best, "best"), (Why::Worst, "worst")] {
        let picked: Vec<&Chosen> = chosen.iter().filter(|c| c.reasons.contains(&why)).collect();
        assert_eq!(picked.len(), 1, "{field}");
        let want = &r[field]["cell"];
        for (k, v) in &picked[0].cell {
            assert_eq!(want[k].as_str(), Some(v.text().as_str()), "{field}.{k}");
        }
    }
}
