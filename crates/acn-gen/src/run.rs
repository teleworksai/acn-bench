//! A generator run (SPEC 050 GEN-21, GEN-22): a sheet into one bundle, on the
//! harness's run path through [`GenDriver`].

use std::collections::BTreeMap;
use std::path::{Path, PathBuf};

use acn_harness::HarnessError;
use acn_harness::agent::Opts;
use acn_harness::run::{HypothesisArg, RunConfig, run_driven_blocking};
use acn_harness::served::LOOPBACK;
use acn_harness::wire::Backend;
use acn_mockllm::profile::Profiles;
use acn_trace::bundle::Written;
use acn_trace::identity::{BuildInfo, Digest, Mode};

use crate::sessions::GenDriver;
use crate::sheet::Sheet;

/// The inputs of `acn gen run` (GEN-22): HAR-50's, with the sheet in place of
/// the workload, backend and model.
#[derive(Debug, Clone)]
pub struct GenConfig {
    pub sheet: PathBuf,
    pub mode: Mode,
    /// `treatment` or `control`.
    pub arm: String,
    pub replicates: u32,
    pub vary: BTreeMap<String, String>,
    pub opts: Opts,
    pub hypothesis: HypothesisArg,
    pub runs_dir: PathBuf,
    pub start_dir: PathBuf,
    pub engine_hash: Digest,
    pub build: BuildInfo,
    /// The mock's profiles; `None` means the embedded ones (MLM-50).
    pub profiles: Option<Profiles>,
}

/// A generator run's bundle, and what it made (GEN-22).
#[derive(Debug)]
pub struct GenWritten {
    pub written: Written,
    pub sessions: u64,
    pub calls: u64,
}

/// Run `cfg` into one bundle, its calls crossing `scenario`'s network when
/// one is given (GEN-20).
pub fn run(cfg: &GenConfig, scenario: Option<&Path>) -> Result<GenWritten, HarnessError> {
    // One read: the bytes parsed are the bytes hashed (CON-27(a)).
    let bytes = std::fs::read(&cfg.sheet)
        .map_err(|e| HarnessError::Workload(format!("{}: {e}", cfg.sheet.display())))?;
    let text = String::from_utf8(bytes.clone())
        .map_err(|e| HarnessError::Workload(format!("{}: {e}", cfg.sheet.display())))?;
    let profiles = match &cfg.profiles {
        Some(p) => p.clone(),
        None => {
            acn_mockllm::profile::embedded().map_err(|e| HarnessError::Config(e.to_string()))?
        }
    };
    let sheet =
        Sheet::parse(&text, &profiles).map_err(|e| HarnessError::Workload(e.to_string()))?;
    let hash = Digest::of(&bytes);
    let mut opts = cfg.opts.clone();
    // GEN-22: in `live` with no endpoint, the harness serves the mock (HAR-26).
    if cfg.mode == Mode::Live && opts.endpoint.is_empty() {
        opts.endpoint = LOOPBACK.to_owned();
    }
    let run = RunConfig {
        workload: cfg.sheet.clone(),
        backend: Backend::Mockllm,
        model: sheet.model.clone(),
        mode: cfg.mode,
        arm: cfg.arm.clone(),
        replicates: cfg.replicates,
        vary: cfg.vary.clone(),
        opts,
        hypothesis: cfg.hypothesis.clone(),
        runs_dir: cfg.runs_dir.clone(),
        start_dir: cfg.start_dir.clone(),
        engine_hash: cfg.engine_hash,
        build: cfg.build.clone(),
        profiles: Some(profiles),
    };
    let driver = GenDriver::new(sheet, hash);
    let written = run_driven_blocking(&run, scenario, &driver)?;
    let (sessions, calls) = driver.counts();
    Ok(GenWritten {
        written,
        sessions,
        calls,
    })
}
