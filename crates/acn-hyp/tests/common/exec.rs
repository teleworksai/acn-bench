//! A loop executor for the tests (LOOP-15): each request is one `sim` run of
//! the real harness on the mock, as `acn-cli`'s executor does, with profiles
//! that cache the smoke workload's short prompts. A hook sees every bundle
//! before it is returned, so a test can fail, redirect or edit around it.
#![allow(dead_code, clippy::unwrap_used, clippy::expect_used, clippy::panic)]

use std::collections::BTreeMap;
use std::path::{Path, PathBuf};

use acn_harness::agent::Opts;
use acn_harness::run::{HypothesisArg, RunConfig, run};
use acn_harness::wire::Backend;
use acn_hyp::loop_run::{Args, Binary, Completed, Executor, LoopError, Request};
use acn_mockllm::profile::{PROFILES_TOML, Profiles};
use acn_trace::identity::{BuildInfo, Digest, Mode};

use super::bundles::build;

/// A candidate with one knob: two cells and one control (the shipped default).
pub const TWO: &str = r#"[poc]
id = "zz"
title = "tool order and the cache"

[hypothesis]
statement = "A stable tool order moves the cached-token ratio."

[varies]
tool_order_stable = { kind = "bool" }

[measures]
primary = ["cached_token_ratio"]
secondary = ["ttft_p50_ms"]

[control]
description = "the shipped default"
config = { tool_order_stable = true }

[design]
search = "grid"
replicates = 4
twin_required = false

[falsifier]
predicate = "max_over_knobs(abs(effect(cached_token_ratio))) < 0.001"
inconclusive_if = "replicates < 4"

[expected]
outcome = "pass"
"#;

pub type Hook = Box<dyn FnMut(usize, &Request, PathBuf) -> Result<PathBuf, String>>;

pub struct Exec {
    pub engine: Digest,
    pub build: BuildInfo,
    pub profiles: Profiles,
    /// Requests run so far, in order.
    pub requests: Vec<Request>,
    pub hook: Option<Hook>,
}

pub fn profiles() -> Profiles {
    // The embedded profiles cache nothing under 1024 tokens, more than the
    // smoke workload sends; a small minimum makes the cache columns real.
    Profiles::parse(
        &PROFILES_TOML
            .replace("min_cacheable_tokens = 1024", "min_cacheable_tokens = 32")
            .replace("increment_tokens = 128", "increment_tokens = 16"),
    )
    .unwrap()
}

impl Exec {
    /// `_dir` names the test's directory for the reader; the harness looks for
    /// the workspace root from the request's `start_dir` (LOOP-15).
    pub fn new(_dir: &Path) -> Self {
        Self::with_build(_dir, "build")
    }

    pub fn with_build(_dir: &Path, tag: &str) -> Self {
        Self {
            engine: Digest::of(super::bundles::ENGINE),
            build: build(tag),
            profiles: profiles(),
            requests: Vec::new(),
            hook: None,
        }
    }

    pub fn hook(
        mut self,
        h: impl FnMut(usize, &Request, PathBuf) -> Result<PathBuf, String> + 'static,
    ) -> Self {
        self.hook = Some(Box::new(h));
        self
    }

    pub fn bin(&self) -> Binary {
        Binary {
            engine_hash: self.engine,
            build_hash: Digest::from_hex(&self.build.build_hash).unwrap(),
        }
    }
}

impl Executor for Exec {
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
            start_dir: r.start_dir.clone(),
            engine_hash: self.engine,
            build: self.build.clone(),
            profiles: Some(self.profiles.clone()),
        };
        let n = self.requests.len();
        self.requests.push(r.clone());
        let dir = run(&cfg).map(|w| w.dir).map_err(|e| e.to_string())?;
        match &mut self.hook {
            Some(h) => h(n, r, dir),
            None => Ok(dir),
        }
    }

    fn check_model(&self, model: &str) -> Result<(), String> {
        self.profiles
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

/// The smoke workload's text.
pub fn smoke() -> String {
    std::fs::read_to_string(concat!(
        env!("CARGO_MANIFEST_DIR"),
        "/../../workloads/harness-smoke.toml"
    ))
    .unwrap()
}

/// A directory holding `zz.toml` (the hypothesis) and `w.toml` (the smoke
/// workload), outside any workspace.
pub fn dir_with(hypothesis: &str) -> tempfile::TempDir {
    let dir = tempfile::tempdir().unwrap();
    std::fs::write(dir.path().join("zz.toml"), hypothesis).unwrap();
    std::fs::write(dir.path().join("w.toml"), smoke()).unwrap();
    dir
}

pub fn args(budget: u64) -> Args {
    Args {
        workloads: vec!["w.toml".into()],
        models: vec!["mock-auto".into()],
        budget,
    }
}

/// `args` with every path made absolute under `dir`.
pub fn args_in(dir: &Path, mut a: Args) -> Args {
    a.workloads = a
        .workloads
        .iter()
        .map(|w| match w.split_once('=') {
            Some((v, p)) => format!("{v}={}", dir.join(p).display()),
            None => dir.join(w).display().to_string(),
        })
        .collect();
    a
}

/// Load `zz.toml` under `dir` and run the loop with `args`.
pub fn run_loop(dir: &Path, a: Args, exec: &mut Exec) -> Result<Completed, LoopError> {
    let h = acn_hyp::load_in(&dir.join("zz.toml"), dir).unwrap();
    let bin = exec.bin();
    acn_hyp::loop_run::run(&h, &args_in(dir, a), &dir.join("runs"), bin, exec)
}

/// The report of a completed loop, parsed.
pub fn report(c: &Completed) -> serde_json::Value {
    serde_json::from_str(&std::fs::read_to_string(&c.report).unwrap()).unwrap()
}

/// Every bundle directory under `runs/`.
pub fn bundles(dir: &Path) -> BTreeMap<String, PathBuf> {
    let runs = dir.join("runs");
    let Ok(rd) = std::fs::read_dir(&runs) else {
        return BTreeMap::new();
    };
    rd.map(|e| e.unwrap().path())
        .filter(|p| p.join("manifest.json").exists())
        .map(|p| (p.file_name().unwrap().to_string_lossy().into_owned(), p))
        .collect()
}
