//! A generator run's identity before it runs (CON-29; SPEC 070 CTL-13).

#![allow(clippy::unwrap_used, clippy::expect_used, clippy::panic)] // CON-19: tests are exempt

mod common;

use acn_gen::run::{plan_run, run};
use acn_trace::identity::Mode;
use common::{SMALL, config};

/// Cites: CON-29, CTL-13, GEN-22
#[test]
fn a_plans_run_id_is_its_runs_in_sim_and_live() {
    for mode in [Mode::Sim, Mode::Live] {
        let dir = tempfile::tempdir().unwrap();
        let mut cfg = config(dir.path(), SMALL);
        cfg.mode = mode;
        if mode == Mode::Live {
            // Fast mock timing, and fail fast: live waits on the wall clock.
            cfg.profiles = Some(
                acn_mockllm::profile::Profiles::parse(
                    &acn_mockllm::profile::PROFILES_TOML
                        .replace("itl_ns = 20_000_000", "itl_ns = 20_000")
                        .replace("itl_jitter_ns = 2_000_000", "itl_jitter_ns = 2_000")
                        .replace("prefill_base_ns = 20_000_000", "prefill_base_ns = 20_000"),
                )
                .unwrap(),
            );
            cfg.opts.request_timeout_ms = 10_000;
            cfg.opts.max_retries = 0;
        }
        let p = plan_run(&cfg, None).unwrap();
        assert!(!cfg.runs_dir.exists());
        let w = run(&cfg, None).unwrap();
        assert_eq!(p.run_id, w.written.run_id, "{mode:?}");
    }
}
