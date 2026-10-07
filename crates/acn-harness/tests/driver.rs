//! The driver seam (SPEC 050 GEN-20, GEN-21): another crate's driver decides a
//! run's sessions on the harness's run path, under its own producer name, with
//! the attributes SPEC 010 lists for that producer and the knobs at their
//! defaults.

#![allow(clippy::unwrap_used, clippy::expect_used, clippy::panic)] // CON-19: tests are exempt

mod common;

use acn_harness::HarnessError;
use acn_harness::agent::Replicate;
use acn_harness::env::Env;
use acn_harness::run::{Driver, RunConfig, run_driven_blocking};
use acn_harness::workload::Workload;
use acn_trace::model::AttrValue;
use common::{fast_smoke, run_fixture, spans};

/// The agent loop under another producer's name, its knobs fixed.
struct Toy;

impl Driver for Toy {
    fn workload(&self, cfg: &RunConfig) -> Result<Workload, HarnessError> {
        Workload::load(&cfg.workload)
    }
    fn producer(&self) -> (&'static str, &'static str) {
        ("acn-gen", "0.1.0")
    }
    fn knobs_fixed(&self) -> bool {
        true
    }
    async fn replicate<E: Env>(
        &self,
        rep: &Replicate<'_, E>,
        seed: i64,
    ) -> Result<(), HarnessError> {
        for t in 0..rep.setup.workload.tasks.len() {
            rep.session(t, seed).await?;
        }
        Ok(())
    }
}

/// Cites: GEN-20, GEN-21, TRC-19
#[test]
fn a_driver_runs_on_the_run_path_under_its_own_producer() {
    let f = run_fixture(&fast_smoke(), "auto");
    let w = run_driven_blocking(&f.cfg, None, &Toy).unwrap();
    let t = common::read(&w.dir);
    // The spans' resource names the driver's producer.
    let names: Vec<&AttrValue> = t
        .resources
        .iter()
        .filter_map(|r| r.attrs.get("service.name"))
        .collect();
    assert!(
        names.contains(&&AttrValue::String("acn-gen".into())),
        "{names:?}"
    );
    assert!(!names.contains(&&AttrValue::String("acn-harness".into())));
    // Only the attributes SPEC 010 lists for it: the knob map yes, the
    // harness's own run options no.
    for s in spans(&t, "acn.session") {
        assert!(s.attrs.contains_key("acn.harness.knobs"));
        for absent in [
            "acn.harness.endpoint",
            "acn.harness.max_retries",
            "acn.harness.retry_base_ms",
            "acn.harness.request_timeout_ms",
        ] {
            assert!(!s.attrs.contains_key(absent), "{absent}");
        }
    }
    assert!(!spans(&t, "chat").is_empty());
}

/// Cites: GEN-21
#[test]
fn a_driver_with_fixed_knobs_refuses_a_knob_vary() {
    let mut f = run_fixture(&fast_smoke(), "auto");
    f.cfg
        .vary
        .insert("tool_order_stable".into(), "false".into());
    let e = run_driven_blocking(&f.cfg, None, &Toy)
        .unwrap_err()
        .to_string();
    assert!(e.contains("GEN-21"), "{e}");
}
