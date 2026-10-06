//! The mock the harness serves itself (SPEC 040 HAR-26): a fresh mock per
//! `live` replicate, built as in `sim`, at an endpoint that does not change
//! the run's identity.

#![allow(clippy::unwrap_used, clippy::expect_used, clippy::panic)] // CON-19: tests are exempt

mod common;

use acn_harness::run::{run, run_with_scenario};
use acn_harness::served::LOOPBACK;
use acn_harness::wire::Backend;
use acn_trace::identity::Mode;
use common::{cached, fast_smoke, run_fixture, spans};

fn manifest(dir: &std::path::Path) -> serde_json::Value {
    serde_json::from_slice(&std::fs::read(dir.join("manifest.json")).unwrap()).unwrap()
}

/// Cites: HAR-26, HAR-25, HAR-23
#[test]
fn a_served_mock_run_has_an_identity_that_does_not_depend_on_its_ports() {
    let ids: Vec<String> = (0..2)
        .map(|_| {
            let mut f = run_fixture(&fast_smoke(), "auto");
            f.cfg.mode = Mode::Live;
            f.cfg.replicates = 2;
            f.cfg.opts.endpoint = LOOPBACK.into();
            let w = run(&f.cfg).unwrap();
            let m = manifest(&w.dir);
            assert_eq!(m["endpoint_host"], "loopback");
            assert_eq!(m["params"]["opt.endpoint"], LOOPBACK);
            assert!(!spans(&common::read(&w.dir), "chat").is_empty());
            w.dir.file_name().unwrap().to_string_lossy().into_owned()
        })
        .collect();
    // Each run served its mocks on ports of its own, and has the same run_id.
    assert_eq!(ids[0], ids[1]);
}

/// Cites: HAR-26
#[test]
fn each_replicate_is_served_a_mock_built_as_in_sim() {
    let reads = |mode: Mode| {
        let mut f = run_fixture(&fast_smoke(), "auto");
        f.cfg.mode = mode;
        f.cfg.replicates = 3;
        if mode == Mode::Live {
            f.cfg.opts.endpoint = LOOPBACK.into();
        }
        let w = run(&f.cfg).unwrap();
        let mut r = cached(&common::read(&w.dir));
        r.sort_unstable();
        r
    };
    // The same profiles and replicate seeds: the same cache accounting, call
    // for call.
    let sim = reads(Mode::Sim);
    assert!(sim.iter().any(|&n| n > 0), "no cache read to compare");
    assert_eq!(sim, reads(Mode::Live));
}

/// Cites: HAR-26, EMU-40
#[test]
fn a_served_mock_sits_behind_the_proxy_of_a_scenario() {
    let mut f = run_fixture(&fast_smoke(), "auto");
    f.cfg.mode = Mode::Live;
    f.cfg.opts.endpoint = LOOPBACK.into();
    let sc = f.dir.path().join("p.toml");
    std::fs::write(
        &sc,
        "schema_version = 1\nname = \"p\"\n\n[[link]]\nname = \"p\"\ndirection = \"up\"\n\n[[link]]\nname = \"p\"\ndirection = \"down\"\n",
    )
    .unwrap();
    let w = run_with_scenario(&f.cfg, Some(&sc)).unwrap();
    let t = common::read(&w.dir);
    assert!(!spans(&t, "acn.link").is_empty());
    assert_eq!(manifest(&w.dir)["endpoint_host"], "loopback");
}

/// Cites: HAR-26
#[test]
fn a_served_mock_is_refused_in_sim_and_on_other_backends() {
    let mut f = run_fixture(&fast_smoke(), "auto");
    f.cfg.opts.endpoint = LOOPBACK.into();
    let e = run(&f.cfg).unwrap_err().to_string();
    assert!(e.contains("HAR-26"), "{e}");

    let mut f = run_fixture(&fast_smoke(), "auto");
    f.cfg.mode = Mode::Live;
    f.cfg.backend = Backend::Vllm;
    f.cfg.opts.endpoint = LOOPBACK.into();
    let e = run(&f.cfg).unwrap_err().to_string();
    assert!(e.contains("HAR-26"), "{e}");
    // Nothing was left behind.
    assert!(
        !f.cfg.runs_dir.exists() || std::fs::read_dir(&f.cfg.runs_dir).unwrap().next().is_none()
    );
}
