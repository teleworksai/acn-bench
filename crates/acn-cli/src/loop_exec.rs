//! The executor `acn loop run` hands the loop runner (LOOP-15): each request
//! becomes one `sim` run of the harness on the mock, with the embedded profiles
//! and every run option at its default (LOOP-10). It decides nothing.

use std::path::{Path, PathBuf};

use acn_harness::agent::Opts;
use acn_harness::run::{HypothesisArg, RunConfig, run};
use acn_harness::wire::Backend;
use acn_hyp::loop_run::{Executor, Request};
use acn_trace::identity::{BuildInfo, Digest, Mode};

pub struct HarnessExecutor {
    pub start_dir: PathBuf,
    pub engine_hash: Digest,
    pub build: BuildInfo,
}

impl Executor for HarnessExecutor {
    fn run(&mut self, r: &Request) -> Result<PathBuf, String> {
        let cfg = RunConfig {
            workload: r.workload.clone(),
            backend: Backend::Mockllm,
            model: r.model.clone(),
            mode: Mode::Sim,
            arm: r.arm.as_str().to_owned(),
            replicates: r.replicates,
            vary: r.vary.clone(),
            opts: Opts::default(),
            hypothesis: HypothesisArg::File(r.hypothesis.clone()),
            runs_dir: r.runs_dir.clone(),
            start_dir: self.start_dir.clone(),
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
