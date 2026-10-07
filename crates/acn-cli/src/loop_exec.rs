//! The executor `acn loop` hands the loop runner (LOOP-15): each request
//! becomes one run of the harness on the mock, with the embedded profiles, in
//! `sim` at L1 and in `live` on the mock the harness serves at L2 (HAR-26),
//! every other run option at its default (LOOP-10). It decides nothing.

use std::path::{Path, PathBuf};

use acn_harness::agent::Opts;
use acn_harness::run::{HypothesisArg, RunConfig, run};
use acn_harness::wire::Backend;
use acn_hyp::loop_run::{Executor, Request};
use acn_trace::identity::{BuildInfo, Digest};

pub struct HarnessExecutor {
    pub engine_hash: Digest,
    pub build: BuildInfo,
}

impl Executor for HarnessExecutor {
    fn run(&mut self, r: &Request) -> Result<PathBuf, String> {
        let cfg = RunConfig {
            workload: r.workload.clone(),
            backend: Backend::Mockllm,
            model: r.model.clone(),
            mode: r.mode,
            arm: r.arm.as_str().to_owned(),
            replicates: r.replicates,
            vary: r.vary.clone(),
            // LOOP-15: every option at its default but the endpoint, which
            // is the served mock's at L2 (HAR-26).
            opts: Opts {
                endpoint: r.endpoint.clone(),
                ..Opts::default()
            },
            hypothesis: HypothesisArg::File(r.hypothesis.clone()),
            runs_dir: r.runs_dir.clone(),
            start_dir: r.start_dir.clone(),
            engine_hash: self.engine_hash,
            build: self.build.clone(),
            profiles: None,
        };
        run(&cfg).map(|w| w.dir).map_err(|e| e.to_string())
    }

    fn check_model(&self, model: &str) -> Result<(), String> {
        let profiles = acn_mockllm::profile::embedded().map_err(|e| e.to_string())?;
        profiles
            .get(model)
            .map(|_| ())
            .ok_or_else(|| "not a mock profile".to_owned())
    }

    fn check_workload(&self, path: &Path) -> Result<(), String> {
        acn_harness::workload::Workload::load(path)
            .map(|_| ())
            .map_err(|e| e.to_string())
    }
}

#[cfg(test)]
mod tests {
    /// Cites: LOOP-15, HAR-26
    #[test]
    fn the_loop_runner_and_the_harness_name_one_served_mock_endpoint() {
        assert_eq!(
            acn_hyp::loop_run::SERVED_MOCK,
            acn_harness::served::LOOPBACK
        );
    }
}
