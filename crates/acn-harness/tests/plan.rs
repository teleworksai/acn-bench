//! A run's identity before it runs (CON-29; SPEC 070 CTL-13): the plan's
//! `run_id` is the run's, in every mode, and a plan writes nothing.

#![allow(clippy::unwrap_used, clippy::expect_used, clippy::panic)] // CON-19: tests are exempt

mod common;

use acn_harness::run::{plan, run_with_scenario};
use acn_harness::served::LOOPBACK;
use acn_trace::identity::Mode;
use common::{fast_smoke, run_fixture};

fn scenario(dir: &std::path::Path) -> std::path::PathBuf {
    let p = dir.join("p.toml");
    std::fs::write(
        &p,
        "schema_version = 1\nname = \"p\"\n\n[[link]]\nname = \"p\"\ndirection = \"up\"\n\n[[link]]\nname = \"p\"\ndirection = \"down\"\n",
    )
    .unwrap();
    p
}

/// Cites: CON-29, CTL-13
#[test]
fn a_plans_run_id_is_its_runs_in_sim_with_and_without_a_scenario() {
    for with in [false, true] {
        let f = run_fixture(&fast_smoke(), "auto");
        let sc = with.then(|| scenario(f.dir.path()));
        let p = plan(&f.cfg, sc.as_deref()).unwrap();
        // Nothing written by the plan.
        assert!(!f.cfg.runs_dir.exists());
        let w = run_with_scenario(&f.cfg, sc.as_deref()).unwrap();
        assert_eq!(p.run_id, w.run_id);
        assert_eq!(p.dir, w.dir);
    }
}

/// Cites: CON-29, CTL-13, HAR-26
#[test]
fn a_plans_run_id_is_its_runs_in_live_on_the_served_mock() {
    let mut f = run_fixture(&fast_smoke(), "auto");
    f.cfg.mode = Mode::Live;
    f.cfg.opts.endpoint = LOOPBACK.into();
    let p = plan(&f.cfg, None).unwrap();
    let w = run_with_scenario(&f.cfg, None).unwrap();
    assert_eq!(p.run_id, w.run_id);
}

/// Cites: CTL-13
#[test]
fn a_plan_refuses_what_a_run_refuses() {
    let mut f = run_fixture(&fast_smoke(), "auto");
    f.cfg.arm = "both".into();
    assert!(plan(&f.cfg, None).is_err());
    let mut f = run_fixture(&fast_smoke(), "auto");
    f.cfg.opts.endpoint = LOOPBACK.into();
    let e = plan(&f.cfg, None).unwrap_err().to_string();
    assert!(e.contains("HAR-26"), "{e}");
    let mut f = run_fixture(&fast_smoke(), "nope");
    f.cfg.model = "nope".into();
    assert!(plan(&f.cfg, None).is_err());
}
