//! The mock the harness serves itself (SPEC 040 HAR-26): a fresh mock per
//! `live` replicate, built as in `sim`, at an endpoint that does not change
//! the run's identity.

#![allow(clippy::unwrap_used, clippy::expect_used, clippy::panic)] // CON-19: tests are exempt

mod common;

use acn_harness::run::{run, run_with_scenario};
use acn_harness::served::LOOPBACK;
use acn_harness::wire::Backend;
use acn_trace::identity::Mode;
use common::{fast_smoke, int, run_fixture, spans};

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

/// Per replicate, every call's output tokens and cache reads, in a canonical
/// order (by call index within a turn, then value). Output lengths are drawn
/// from the mock's seeded stream, so they show which mock answered.
fn per_replicate(t: &acn_trace::model::Trace) -> Vec<Vec<(i64, i64)>> {
    let replicate: std::collections::BTreeMap<_, _> = spans(t, "acn.session")
        .iter()
        .map(|s| (s.trace_id, int(s, "acn.replicate").unwrap()))
        .collect();
    let mut out: std::collections::BTreeMap<i64, Vec<(i64, i64, i64)>> = Default::default();
    for c in spans(t, "chat") {
        out.entry(replicate[&c.trace_id]).or_default().push((
            int(c, "acn.call.index").unwrap_or(c.start_ns),
            int(c, "gen_ai.usage.output_tokens").unwrap(),
            int(c, "acn.cache.read_tokens").unwrap(),
        ));
    }
    out.into_values()
        .map(|mut v| {
            v.sort_unstable();
            v.into_iter().map(|(_, o, r)| (o, r)).collect()
        })
        .collect()
}

/// Cites: HAR-26
#[test]
fn each_replicate_is_served_a_fresh_mock_built_as_in_sim() {
    let calls = |mode: Mode| {
        let mut f = run_fixture(&fast_smoke(), "auto");
        f.cfg.mode = mode;
        f.cfg.replicates = 3;
        if mode == Mode::Live {
            f.cfg.opts.endpoint = LOOPBACK.into();
        }
        let w = run(&f.cfg).unwrap();
        per_replicate(&common::read(&w.dir))
    };
    let sim = calls(Mode::Sim);
    assert_eq!(sim.len(), 3);
    assert!(sim.iter().flatten().any(|&(_, r)| r > 0), "no cache read");
    // Replicates draw different lengths, so a mock shared by the replicates,
    // whose stream would run on from one replicate into the next, would not
    // give these numbers.
    assert_ne!(sim[0], sim[1]);
    assert_eq!(sim, calls(Mode::Live));
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
    assert!(
        !f.cfg.runs_dir.exists() || std::fs::read_dir(&f.cfg.runs_dir).unwrap().next().is_none()
    );

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
