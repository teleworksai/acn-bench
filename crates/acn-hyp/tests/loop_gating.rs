//! LOOP-4: the gates between layers. A twin starts only from an L1 report that
//! regenerates. A provider run starts only from an L2 verdict that reads every
//! L1 bundle, twins every decision cell of the L1 verdict, and records no
//! `twin_failed`, unless the file waives the twin with `twin_required = false`.

#![allow(clippy::unwrap_used, clippy::expect_used, clippy::panic)] // CON-19: tests are exempt

mod common;

use acn_hyp::Hypothesis;
use acn_hyp::evidence::{promote_gate, twin_gate};
use acn_hyp::loop_run::Code;
use acn_hyp::read::BundleData;
use acn_hyp::verdict::{Verdict, verdict};
use common::bundles::{Spec, alt, bundle, engine};
use common::exec::{Exec, TWO, args, dir_with, run_loop};

/// Cites: LOOP-4
#[test]
fn a_twin_starts_only_from_a_report_that_regenerates() {
    let dir = dir_with(TWO);
    let d = dir.path();
    let c = run_loop(d, args(10), &mut Exec::new(d)).unwrap();
    let mut ex = Exec::new(d);
    let bin = ex.bin();
    let g = twin_gate(&c.report, bin, &mut ex).unwrap();
    assert!(g.identical());
    // A bundle that no longer matches what regenerates.
    let first = c.run_ids[0].to_hex();
    std::fs::write(d.join("runs").join(&first).join("spans.parquet"), b"x").unwrap();
    let e = twin_gate(&c.report, bin, &mut Exec::new(d)).unwrap_err();
    assert_eq!(e.code, Code::TwinRefused, "{e}");
    assert!(
        e.message.contains(&first) && e.message.contains("differ"),
        "a regeneration that completed and differs: {e}"
    );
    // A report another build made.
    let mut other = Exec::with_build(d, "other");
    let other_bin = other.bin();
    let e = twin_gate(&c.report, other_bin, &mut other).unwrap_err();
    assert_eq!(e.code, Code::TwinRefused, "{e}");
    assert!(e.message.contains("not_regenerable_with_this_build"), "{e}");
}

/// BASE with a twin required within 0.03 of `cached_token_ratio`.
fn twin_file() -> Hypothesis {
    twin_file_within("0.03")
}

fn twin_file_within(tol: &str) -> Hypothesis {
    let text = common::BASE.replace(
        "twin_required = false",
        &format!("twin_required = true\nsim_live_tolerance = {{ cached_token_ratio = {{ abs = {tol} }} }}"),
    );
    let (h, _dir) = common::candidate(&text, "t1");
    h.unwrap()
}

/// BASE's six arms in `mode`: knob=true moves the ratio by 0.3 when fast and
/// 0.35 when slow (so the slow cell decides the maximum), and `shift` adds to
/// every value; `keep` decides which arms are made, by knob value (`"control"`
/// for a control) and mode.
fn arms(
    h: &Hypothesis,
    mode: &str,
    shift: f64,
    keep: &dyn Fn(&str, &str) -> bool,
) -> Vec<BundleData> {
    let base = alt(4, 0.40, 0.46);
    let shifted = |e: f64| base.iter().map(|x| x.map(|x| x + e + shift)).collect();
    let mut out = Vec::new();
    for m in ["fast", "slow"] {
        if !keep("control", m) {
            continue;
        }
        out.push(bundle(
            h,
            &Spec::new(
                &format!("{mode}-c-{m}"),
                &[("knob", "false"), ("mode", m)],
                "control",
                shifted(0.0),
            )
            .mode(mode),
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
                    &format!("{mode}-t-{k}-{m}"),
                    &[("knob", k), ("mode", m)],
                    "treatment",
                    shifted(e),
                )
                .mode(mode),
            ));
        }
    }
    out
}

fn every(_: &str, _: &str) -> bool {
    true
}

fn judge(h: &Hypothesis, b: Vec<BundleData>) -> Verdict {
    verdict(h, b, engine()).unwrap()
}

/// Cites: LOOP-4
#[test]
fn a_provider_run_needs_an_l2_verdict_that_twins_every_decision_cell() {
    let h = twin_file();
    let sim = arms(&h, "sim", 0.0, &every);
    let l1 = judge(&h, sim.clone());
    assert!(!l1.slices[0].eval.decision_cells.is_empty());
    let refused = |l2: Option<&Verdict>| -> String {
        let e = promote_gate(&h, &l1, l2).unwrap_err();
        assert_eq!(e.code, Code::PromoteRefused, "{e}");
        e.message
    };
    // No L2 verdict at all.
    assert!(refused(None).contains("no L2 verdict"));
    // The L1 verdict given as if it were L2.
    assert!(refused(Some(&l1)).contains("not L2"));
    // Live twins within the tolerance, for every cell: the gate opens.
    let mut b = sim.clone();
    b.extend(arms(&h, "live", 0.01, &every));
    let l2 = judge(&h, b);
    promote_gate(&h, &l1, Some(&l2)).unwrap();
    // Twins outside the tolerance: `twin_failed`.
    let mut b = sim.clone();
    b.extend(arms(&h, "live", 0.2, &every));
    let far = judge(&h, b);
    assert!(refused(Some(&far)).contains("twin_failed"));
    // A live control without its decision cell's live treatment: the verdict
    // itself records `twin_failed` (HYP-22).
    let mut b = sim.clone();
    b.extend(arms(&h, "live", 0.01, &|k, _| k != "true"));
    let missing = judge(&h, b);
    assert!(refused(Some(&missing)).contains("twin_failed"));
    // No live arm at all where the verdict decides: no `twin_failed`, only a
    // `partially-twinned` label, and the gate still refuses (LOOP-4).
    let mut b = sim.clone();
    b.extend(arms(&h, "live", 0.01, &|_, m| m == "fast"));
    let partial = judge(&h, b);
    assert!(
        partial
            .labels
            .contains(&acn_hyp::verdict::Label::PartiallyTwinned),
        "{:?} {:?}",
        partial.labels,
        partial.reasons
    );
    let m = refused(Some(&partial));
    assert!(
        m.contains("decision cells") && m.contains("knob=true,mode=slow"),
        "{m}"
    );
    // An L2 verdict over other sim bundles than the L1 verdict read.
    let mut b = arms(&h, "sim", 0.0, &|k, m| !(k == "false" && m == "slow"));
    // (the other set lacks one sim treatment the L1 verdict read)
    b.extend(arms(&h, "live", 0.01, &every));
    let other = judge(&h, b);
    assert!(refused(Some(&other)).contains("not exactly those of the L1 verdict"));
    // An L2 verdict over more sim bundles than the L1 verdict read: not its twin.
    let fewer = arms(&h, "sim", 0.0, &|k, m| !(k == "false" && m == "slow"));
    let l1_fewer = judge(&h, fewer);
    let e = promote_gate(&h, &l1_fewer, Some(&l2)).unwrap_err();
    assert!(e.message.contains("not exactly those"), "{e}");
    // An L2 verdict passed as the L1 one.
    let e = promote_gate(&h, &l2, Some(&l2)).unwrap_err();
    assert!(e.message.contains("not an L1 verdict"), "{e}");
    // A verdict of another hypothesis: a copy with a looser tolerance, under
    // which the far twins pass, does not open the gate for this one.
    let loose = twin_file_within("0.5");
    let mut b = arms(&loose, "sim", 0.0, &every);
    b.extend(arms(&loose, "live", 0.2, &every));
    let l2_loose = judge(&loose, b);
    assert!(
        !l2_loose
            .reasons
            .iter()
            .any(|r| r.id == acn_hyp::verdict::ReasonId::TwinFailed)
    );
    assert!(refused(Some(&l2_loose)).contains("is not a verdict of"));
    let l1_loose = judge(&loose, arms(&loose, "sim", 0.0, &every));
    let e = promote_gate(&h, &l1_loose, Some(&l2)).unwrap_err();
    assert!(e.message.contains("is not a verdict of"), "{e}");
}

/// Cites: LOOP-4
#[test]
fn a_network_free_hypothesis_waives_the_twin() {
    let (h, _dir) = common::candidate(common::BASE, "t1");
    let h = h.unwrap();
    assert!(!h.design().twin_required);
    let l1 = judge(&h, arms(&h, "sim", 0.0, &every));
    promote_gate(&h, &l1, None).unwrap();
}
