//! The L2 twin (SPEC 085 LOOP-12, LOOP-16). The cells a twin runs are the
//! decision cells of the loop's final verdict and its *k* best and worst
//! cells, chosen from that verdict alone.

#![allow(clippy::unwrap_used, clippy::expect_used, clippy::panic)] // CON-19: tests are exempt

mod common;

use acn_hyp::Hypothesis;
use acn_hyp::loop_run::Code;
use acn_hyp::loop_run::{Completed, SERVED_MOCK};
use acn_hyp::loop_twin::{Chosen, Why, choose};
use acn_hyp::loop_twin::{Twinned, twin};
use acn_hyp::read::BundleData;
use acn_hyp::verdict::{Verdict, verdict};
use acn_trace::identity::Mode;
use common::bundles::{Spec, alt, bundle, engine};
use common::exec::{Exec, TWO, args, dir_with, dir_with_fast, run_loop};
use std::path::{Path, PathBuf};

fn base() -> Hypothesis {
    let (h, _dir) = common::candidate(common::BASE, "t1");
    h.unwrap()
}

/// BASE's control and four treatment cells in `sim`: knob=true moves the
/// ratio by 0.3 when fast and 0.35 when slow, knob=false by nothing, so the
/// slow knob=true cell decides `max_over_knobs`. `keep` decides which
/// treatment cells are made, by knob and mode.
fn arms(h: &Hypothesis, keep: &dyn Fn(&str, &str) -> bool) -> Vec<BundleData> {
    arms_in(h, "sim", keep)
}

fn arms_in(h: &Hypothesis, mode: &str, keep: &dyn Fn(&str, &str) -> bool) -> Vec<BundleData> {
    let base = alt(4, 0.40, 0.46);
    let shifted = |e: f64| base.iter().map(|x| x.map(|x| x + e)).collect();
    let mut out = Vec::new();
    for m in ["fast", "slow"] {
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
    let l1: Verdict = verdict(&h, bundles, ex.bin().engine_hash).unwrap();
    assert_eq!(l1.verdict_id, c.verdict_id);
    let chosen = choose(&h, &l1, 1).unwrap();
    for (why, field) in [(Why::Best, "best"), (Why::Worst, "worst")] {
        let picked: Vec<&Chosen> = chosen.iter().filter(|c| c.reasons.contains(&why)).collect();
        assert_eq!(picked.len(), 1, "{field}");
        assert_eq!(r[field]["slice"].as_str(), Some(picked[0].slice.as_str()));
        let effect =
            l1.slices[picked[0].slice_index].effects[&picked[0].cell_index]["cached_token_ratio"]
                .value;
        assert_eq!(r[field]["effect"].as_f64(), effect, "{field}");
        let want = &r[field]["cell"];
        for (k, v) in &picked[0].cell {
            assert_eq!(want[k].as_str(), Some(v.text().as_str()), "{field}.{k}");
        }
    }
}

/// Cites: LOOP-12
#[test]
fn a_twin_starts_only_from_an_l1_verdict_of_its_own_hypothesis() {
    let h = base();
    let l1 = judge(&h, arms(&h, &|_, _| true));
    let other = {
        let (h, _dir) = common::candidate(
            &common::BASE.replace("title = \"a test\"", "title = \"other\""),
            "t1",
        );
        h.unwrap()
    };
    let e = choose(&other, &l1, 1).unwrap_err();
    assert_eq!(e.code, Code::TwinRefused, "{e}");
    // A verdict over sim and live bundles is L2, not a loop's final verdict.
    let mut both = arms(&h, &|_, _| true);
    both.extend(arms_in(&h, "live", &|_, _| true));
    let l2 = verdict(&h, both, engine()).unwrap();
    let e = choose(&h, &l2, 1).unwrap_err();
    assert_eq!(e.code, Code::TwinRefused, "{e}");
}

/// A loop of TWO on the fast profiles, and its directory.
fn fast_loop() -> (tempfile::TempDir, Completed) {
    let dir = dir_with_fast(TWO);
    let c = run_loop(dir.path(), args(10), &mut Exec::fast(dir.path())).unwrap();
    (dir, c)
}

fn twin_with(
    d: &Path,
    c: &Completed,
    top: u32,
    ex: &mut Exec,
) -> Result<Twinned, acn_hyp::loop_run::LoopError> {
    let bin = ex.bin();
    twin(&d.join("runs"), &c.loop_id.to_hex(), top, bin, ex)
}

fn json(p: &Path) -> serde_json::Value {
    serde_json::from_str(&std::fs::read_to_string(p).unwrap()).unwrap()
}

fn live_requests(ex: &Exec) -> Vec<&acn_hyp::loop_run::Request> {
    ex.requests
        .iter()
        .filter(|r| r.mode == Mode::Live)
        .collect()
}

fn copy_dir(from: &Path, to: &Path) {
    std::fs::create_dir_all(to).unwrap();
    for e in std::fs::read_dir(from).unwrap() {
        let e = e.unwrap();
        let t = to.join(e.file_name());
        if e.file_type().unwrap().is_dir() {
            copy_dir(&e.path(), &t);
        } else {
            std::fs::copy(e.path(), t).unwrap();
        }
    }
}

/// An executor whose live bundles are byte copies of those under
/// `runs/<from>/`: a twin that measures exactly what an earlier one did.
fn replaying(d: &Path, from: &str) -> Exec {
    let src = d.join("runs").join(from);
    Exec::fast(d).hook(move |_, r, got: PathBuf| {
        if r.mode == Mode::Live {
            let name = got.file_name().unwrap().to_owned();
            std::fs::remove_dir_all(&got).unwrap();
            copy_dir(&src.join(name), &got);
        }
        Ok(got)
    })
}

/// Cites: LOOP-12, LOOP-16, LOOP-2, LOOP-1, LOOP-15, HAR-26
#[test]
fn a_twin_runs_each_l1_bundle_once_in_live_and_writes_an_object_that_verifies() {
    let (dir, c) = fast_loop();
    let d = dir.path();
    let runs = d.join("runs");
    let mut ex = Exec::fast(d);
    let t = twin_with(d, &c, 1, &mut ex).unwrap();
    // TWO: two cells sharing one control. Every arm runs once in live, on
    // the served mock, under runs/live/1.
    let live = live_requests(&ex);
    assert_eq!(live.len(), 3, "{live:?}");
    for r in &live {
        assert_eq!(r.endpoint, SERVED_MOCK);
        assert_eq!(
            r.runs_dir,
            std::fs::canonicalize(&runs).unwrap().join("live").join("1")
        );
    }
    assert_eq!(
        live.iter().filter(|r| r.arm.as_str() == "control").count(),
        1
    );
    assert_eq!(t.run_ids.len(), 3);
    assert!(t.run_ids.windows(2).all(|w| w[0].0 < w[1].0));
    assert!(t.twinned, "every decision cell twinned");
    assert!(!t.twin_failed, "no tolerance, so no twin_failed");
    // The object.
    let o = json(&t.twin);
    assert_eq!(o["format"], "acn-bench/loop-twin/v1");
    assert_eq!(o["layer"], "L2");
    assert_eq!(o["loop_id"], c.loop_id.to_hex());
    assert_eq!(o["l1_verdict_id"], c.verdict_id.to_hex());
    assert_eq!(o["live_dir"], "live/1");
    assert_eq!(o["top"], 1);
    assert_eq!(o["verdict_id"], t.verdict_id.to_hex());
    let l1: Vec<String> = c.run_ids.iter().map(|d| d.to_hex()).collect();
    for cell in o["cells"].as_array().unwrap() {
        for arm in ["treatment", "control"] {
            let a = &cell["arms"][arm];
            assert!(l1.contains(&a["derived_from"].as_str().unwrap().to_owned()));
            let id = a["run_id"].as_str().unwrap();
            assert!(runs.join("live/1").join(id).join("manifest.json").exists());
        }
    }
    assert!(t.twin.ends_with(format!(
        "loop/{}/twin/{}/twin.json",
        c.loop_id.to_hex(),
        t.verdict_id.to_hex()
    )));
    assert!(
        runs.join("verdicts")
            .join(t.verdict_id.to_hex())
            .join("verdict.json")
            .exists()
    );
    // The chain verifies from the loop and from the L2 verdict (LOOP-2).
    let mut v = Exec::fast(d);
    let bin = v.bin();
    let k = acn_hyp::evidence::verify(&runs, &c.loop_id.to_hex(), bin, &mut v).unwrap();
    assert!(k.ok(), "{:?}", k.findings);
    assert_eq!(k.twins.len(), 1);
    let k = acn_hyp::evidence::verify(&runs, &t.verdict_id.to_hex(), bin, &mut v).unwrap();
    assert!(k.ok(), "{:?}", k.findings);
    assert_eq!(k.loops, [c.loop_id.to_hex()]);
    // A second twin measures afresh, beside the first.
    let mut ex2 = Exec::fast(d);
    let t2 = twin_with(d, &c, 1, &mut ex2).unwrap();
    assert_eq!(json(&t2.twin)["live_dir"], "live/2");
    assert_ne!(t2.verdict_id, t.verdict_id);
    assert!(t.twin.exists());
    let k = acn_hyp::evidence::verify(&runs, &c.loop_id.to_hex(), bin, &mut v).unwrap();
    assert!(k.ok(), "{:?}", k.findings);
    assert_eq!(k.twins.len(), 2);
}

/// Cites: LOOP-12, LOOP-4
#[test]
fn a_twin_is_refused_before_any_run_on_a_report_that_does_not_regenerate() {
    let (dir, c) = fast_loop();
    let d = dir.path();
    let runs = d.join("runs");
    let first = c.run_ids[0].to_hex();
    std::fs::write(runs.join(&first).join("spans.parquet"), b"x").unwrap();
    let mut ex = Exec::fast(d);
    let e = twin_with(d, &c, 1, &mut ex).unwrap_err();
    assert_eq!(e.code, Code::TwinRefused, "{e}");
    assert!(live_requests(&ex).is_empty());
    assert!(!runs.join("live").exists());
    // No report, and no id.
    let bin = ex.bin();
    let e = twin(&runs, &"0".repeat(64), 1, bin, &mut ex).unwrap_err();
    assert_eq!(e.code, Code::NoLoopReport, "{e}");
    let e = twin(&runs, "nope", 1, bin, &mut ex).unwrap_err();
    assert_eq!(e.code, Code::BadId, "{e}");
}

/// Cites: LOOP-16, LOOP-12
#[test]
fn a_twin_object_is_never_overwritten_and_its_verdict_is_kept_or_conflicts() {
    let (dir, c) = fast_loop();
    let d = dir.path();
    let runs = d.join("runs");
    let t = twin_with(d, &c, 1, &mut Exec::fast(d)).unwrap();
    let original = std::fs::read(&t.twin).unwrap();
    // The same live bundles again: the same L2 verdict, so the object exists.
    let e = twin_with(d, &c, 1, &mut replaying(d, "live/1")).unwrap_err();
    assert_eq!(e.code, Code::TwinExists, "{e}");
    assert_eq!(std::fs::read(&t.twin).unwrap(), original);
    // Without the object, its identical verdict is kept and it is written again.
    let tdir = t.twin.parent().unwrap().to_path_buf();
    std::fs::remove_dir_all(&tdir).unwrap();
    let again = twin_with(d, &c, 1, &mut replaying(d, "live/1")).unwrap();
    assert_eq!(again.verdict_id, t.verdict_id);
    assert_eq!(json(&again.twin)["live_dir"], "live/3");
    // A verdict with other bytes is a conflict, and no object is written.
    std::fs::remove_dir_all(&tdir).unwrap();
    let vpath = runs
        .join("verdicts")
        .join(t.verdict_id.to_hex())
        .join("verdict.json");
    let mut bytes = std::fs::read(&vpath).unwrap();
    bytes.insert(0, b' ');
    std::fs::write(&vpath, bytes).unwrap();
    let e = twin_with(d, &c, 1, &mut replaying(d, "live/1")).unwrap_err();
    assert_eq!(e.code, Code::VerdictConflict, "{e}");
    assert!(!tdir.exists());
}

/// Cites: LOOP-12
#[test]
fn a_failed_live_run_aborts_the_twin_and_writes_nothing() {
    let (dir, c) = fast_loop();
    let d = dir.path();
    let runs = d.join("runs");
    let verdicts = std::fs::read_dir(runs.join("verdicts")).unwrap().count();
    let mut ex = Exec::fast(d).hook(|_, r, got| {
        if r.mode == Mode::Live {
            Err("the served mock did not start".into())
        } else {
            Ok(got)
        }
    });
    let e = twin_with(d, &c, 1, &mut ex).unwrap_err();
    assert_eq!(e.code, Code::ExecutorFailed, "{e}");
    assert!(
        !runs
            .join("loop")
            .join(c.loop_id.to_hex())
            .join("twin")
            .exists()
    );
    assert_eq!(
        std::fs::read_dir(runs.join("verdicts")).unwrap().count(),
        verdicts
    );
}

/// Cites: LOOP-16, LOOP-2, LOOP-1
#[test]
fn evidence_verify_fails_on_a_twin_object_that_does_not_hold() {
    let (dir, c) = fast_loop();
    let d = dir.path();
    let runs = d.join("runs");
    let t = twin_with(d, &c, 1, &mut Exec::fast(d)).unwrap();
    let original = std::fs::read_to_string(&t.twin).unwrap();
    let mut v = Exec::fast(d);
    let bin = v.bin();
    let mut check = |edit: &dyn Fn(&str) -> String, want: Code| {
        std::fs::write(&t.twin, edit(&original)).unwrap();
        let k = acn_hyp::evidence::verify(&runs, &c.loop_id.to_hex(), bin, &mut v).unwrap();
        assert!(!k.ok());
        assert!(
            k.findings.iter().any(|f| f.code == want),
            "{want:?}: {:?}",
            k.findings
        );
        std::fs::write(&t.twin, &original).unwrap();
    };
    // A derived_from the report does not name.
    let some = c.run_ids[0].to_hex();
    check(
        &|s| s.replacen(&some, &"a".repeat(64), 1),
        Code::TwinMismatch,
    );
    // A layer that is not the derived one.
    check(
        &|s| s.replace("\"layer\":\"L2\"", "\"layer\":\"L1\""),
        Code::LayerMismatch,
    );
    // An L2 verdict that does not recompute.
    let vpath = runs
        .join("verdicts")
        .join(t.verdict_id.to_hex())
        .join("verdict.json");
    let vbytes = std::fs::read(&vpath).unwrap();
    std::fs::write(&vpath, [b" ".as_slice(), &vbytes].concat()).unwrap();
    let k = acn_hyp::evidence::verify(&runs, &t.verdict_id.to_hex(), bin, &mut v).unwrap();
    assert!(
        k.findings.iter().any(|f| f.code == Code::VerdictMismatch),
        "{:?}",
        k.findings
    );
}

/// Cites: LOOP-12, LOOP-16, HYP-22
#[test]
fn a_divergent_twin_completes_and_records_twin_failed() {
    let required = TWO.replace(
        "twin_required = false",
        "twin_required = true\nsim_live_tolerance = { cached_token_ratio = { abs = 0.01 } }",
    );
    let dir = dir_with_fast(&required);
    let d = dir.path();
    let c = run_loop(d, args(10), &mut Exec::fast(d)).unwrap();
    // The same mock in both modes: within tolerance.
    let t = twin_with(d, &c, 1, &mut Exec::fast(d)).unwrap();
    assert!(t.twinned && !t.twin_failed, "{t:?}");
    let o = json(&t.twin);
    let cells = o["divergence"][0]["cells"].as_array().unwrap();
    assert!(!cells.is_empty());
    for cell in cells {
        let q = &cell["quantities"]["cached_token_ratio"];
        assert_eq!(q["tolerance"]["abs"], 0.01);
        assert_eq!(q["within"], true, "{q}");
    }
    // A live mock that caches nothing: the simulator diverges, which the twin
    // records and does not refuse.
    let mut ex = Exec::fast(d);
    ex.live_profiles = Some(
        acn_mockllm::profile::Profiles::parse(
            &acn_mockllm::profile::PROFILES_TOML
                .replace(
                    "min_cacheable_tokens = 1024",
                    "min_cacheable_tokens = 1000000",
                )
                .replace("itl_ns = 20_000_000", "itl_ns = 20_000")
                .replace("itl_jitter_ns = 2_000_000", "itl_jitter_ns = 2_000")
                .replace("prefill_base_ns = 20_000_000", "prefill_base_ns = 20_000"),
        )
        .unwrap(),
    );
    let t = twin_with(d, &c, 1, &mut ex).unwrap();
    assert!(t.twin_failed, "{t:?}");
    // The object shows where: a cached-token ratio outside its tolerance.
    let o = json(&t.twin);
    let outside = o["divergence"]
        .as_array()
        .unwrap()
        .iter()
        .flat_map(|s| s["cells"].as_array().unwrap())
        .any(|c| c["quantities"]["cached_token_ratio"]["within"] == false);
    assert!(outside, "{o}");
}
