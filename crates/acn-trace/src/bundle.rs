//! The run bundle (TRC-22, TRC-23): `runs/<run_id>/` holding the tables, an optional
//! `sidecar/`, `logs/`, and `manifest.json`, which is written last and names the
//! BLAKE3 of every other file outside `logs/`. `bundle_digest`, the BLAKE3 of the
//! manifest, names the data; `run_id` names the experiment.
//!
//! A bundle is created only with a [`Preflight`] (CON-28), never into an existing
//! directory (CON-29), and [`verify`] recomputes everything the manifest claims.

use std::collections::BTreeMap;
use std::path::{Path, PathBuf};

use serde::{Deserialize, Serialize};
use serde_json::Value as Json;
use walkdir::WalkDir;

use crate::env::{Preflight, RunHypothesis};
use crate::identity::{self, BuildInfo, Digest, HypStatus, Mode, RunIdentity, RunParams};
use crate::ingest;
use crate::model::{AttrValue, Trace};
use crate::parquet_io::{self, EVENTS, LINKS, RESOURCES, SPANS};
use crate::schema;

/// The manifest's file name.
pub const MANIFEST: &str = "manifest.json";
/// The directory exempt from byte identity and from the manifest (TRC-23, TRC-24).
pub const LOGS: &str = "logs";
/// The optional side-channel directory (TRC-22, §8).
pub const SIDECAR: &str = "sidecar";
/// The `acn.backend` value of the mock backend (CON-26).
pub const MOCK_BACKEND: &str = "mockllm";
/// The hypothesis id of a run with none (CON-27(a)).
pub const NO_HYPOTHESIS: &str = "none";

/// A bundle that cannot be written or does not verify.
#[derive(Debug, thiserror::Error)]
pub enum BundleError {
    #[error("{0}")]
    Invalid(String),
    /// CON-29: a bundle is never written into an existing directory.
    #[error("refusing to write: {0}")]
    Refused(String),
    #[error("io error at {path}: {source}")]
    Io {
        path: PathBuf,
        source: std::io::Error,
    },
    #[error(transparent)]
    Identity(#[from] identity::IdentityError),
    #[error(transparent)]
    Write(#[from] parquet_io::WriteError),
    #[error(transparent)]
    Schema(#[from] schema::SchemaError),
    #[error("directory walk failed: {0}")]
    Walk(#[from] walkdir::Error),
    #[error(transparent)]
    Ingest(#[from] ingest::IngestError),
}

type Result<T> = std::result::Result<T, BundleError>;

fn invalid<T>(message: impl Into<String>) -> Result<T> {
    Err(BundleError::Invalid(message.into()))
}

fn io(path: &Path) -> impl FnOnce(std::io::Error) -> BundleError + '_ {
    move |source| BundleError::Io {
        path: path.to_path_buf(),
        source,
    }
}

/// `hypothesis {id, status, hash}` of the manifest.
#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
#[serde(deny_unknown_fields)]
pub struct ManifestHypothesis {
    pub id: String,
    pub status: String,
    pub hash: String,
}

/// `manifest.json` (TRC-22). Optional fields are absent, never null.
#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
#[serde(deny_unknown_fields)]
pub struct Manifest {
    pub run_id: String,
    /// Decimal, because JSON readers lose integers above 2^53 (CON-30).
    pub seed: String,
    pub mode: String,
    pub backend: String,
    pub model: String,
    /// CON-26: required for every backend but the mock; absent for the mock in `sim`.
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub endpoint_host: Option<String>,
    pub scenario_hash: String,
    pub workload_hash: String,
    pub hypothesis: ManifestHypothesis,
    pub engine_hash: String,
    /// `build_hash` and its components (CON-31).
    pub build: BuildInfo,
    /// The key/value pairs of CON-29.
    pub params: BTreeMap<String, String>,
    /// CON-5(d): the seed-derived order of cells, arms and replicates; `live` and
    /// `netem` only.
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub execution_order: Option<Vec<String>>,
    pub semconv_version: String,
    /// The `service.name` of every resource, sorted.
    pub producers: Vec<String>,
    /// The only wall-clock time in a bundle (TRC-26); `live` and `netem` only.
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub started_at: Option<String>,
    pub replicates: u32,
    /// Path relative to the bundle → BLAKE3, for every file but the manifest and
    /// `logs/` (TRC-23).
    pub files: BTreeMap<String, String>,
}

/// The canonical text of a JSON value (TRC-23): sorted keys, no insignificant
/// whitespace, integers in decimal. A float is refused rather than handed to
/// serde_json's formatter, which no longer writes the form CON-27(c) names
/// (ADR-13); the manifest has none.
fn canonical(v: &Json, out: &mut String) -> Result<()> {
    match v {
        Json::Null => out.push_str("null"),
        Json::Bool(b) => out.push_str(if *b { "true" } else { "false" }),
        Json::Number(n) => {
            if let Some(i) = n.as_i64() {
                out.push_str(&i.to_string());
            } else if let Some(u) = n.as_u64() {
                out.push_str(&u.to_string());
            } else {
                return invalid(format!("the manifest holds no floats; found {n}"));
            }
        }
        Json::String(s) => match serde_json::to_string(s) {
            Ok(t) => out.push_str(&t),
            Err(e) => return invalid(e.to_string()),
        },
        Json::Array(a) => {
            out.push('[');
            for (i, x) in a.iter().enumerate() {
                if i > 0 {
                    out.push(',');
                }
                canonical(x, out)?;
            }
            out.push(']');
        }
        Json::Object(m) => {
            let mut keys: Vec<&String> = m.keys().collect();
            keys.sort();
            out.push('{');
            for (i, k) in keys.into_iter().enumerate() {
                if i > 0 {
                    out.push(',');
                }
                match serde_json::to_string(k) {
                    Ok(t) => out.push_str(&t),
                    Err(e) => return invalid(e.to_string()),
                }
                out.push(':');
                canonical(&m[k], out)?;
            }
            out.push('}');
        }
    }
    Ok(())
}

impl Manifest {
    /// The manifest's bytes: canonical JSON and one trailing newline (TRC-23).
    pub fn to_bytes(&self) -> Result<Vec<u8>> {
        let v = serde_json::to_value(self).map_err(|e| BundleError::Invalid(e.to_string()))?;
        let mut out = String::new();
        canonical(&v, &mut out)?;
        out.push('\n');
        Ok(out.into_bytes())
    }

    /// Check what the manifest claims about itself and return the recomputed
    /// `run_id`: the identity (CON-29), the build hash (CON-31), the rules of
    /// CON-26 and TRC-22, and that each field the parameters repeat agrees with them.
    pub fn validate(&self) -> Result<Digest> {
        let seed: u64 = self
            .seed
            .parse()
            .map_err(|_| BundleError::Invalid(format!("seed `{}` is not a u64", self.seed)))?;
        if seed.to_string() != self.seed {
            return invalid(format!("seed `{}` is not in decimal text form", self.seed));
        }
        check_seed(seed)?;
        let mode = Mode::parse(&self.mode)?;
        let status = HypStatus::parse(&self.hypothesis.status)?;
        let hypothesis_hash = Digest::from_hex(&self.hypothesis.hash)?;
        let none = self.hypothesis.id == NO_HYPOTHESIS;
        if none != (hypothesis_hash == Digest::ZERO) || (none && status != HypStatus::Candidate) {
            return invalid(
                "a run with no hypothesis has id `none`, 32 zero bytes and status `candidate`, and only such a run (CON-27(a))",
            );
        }
        if self.hypothesis.id.is_empty() {
            return invalid("hypothesis id must not be empty");
        }
        for (key, value) in [
            ("backend", self.backend.as_str()),
            ("model", self.model.as_str()),
            ("hyp_status", self.hypothesis.status.as_str()),
            ("replicates", &self.replicates.to_string()),
        ] {
            if self.params.get(key).map(String::as_str) != Some(value) {
                return invalid(format!(
                    "params.{key} must equal the manifest's own value `{value}` (CON-29)"
                ));
            }
        }
        let inv = schema::inventory()?;
        identity::check_pairs(&self.params, &identity::options(&inv))?;
        let mock = self.backend == MOCK_BACKEND;
        match (&self.endpoint_host, mock, mode) {
            (Some(_), true, Mode::Sim) => {
                return invalid("the mock backend in sim has no endpoint host (TRC-22)");
            }
            (None, false, _) => {
                return invalid(format!(
                    "backend `{}` must record its endpoint host (CON-26)",
                    self.backend
                ));
            }
            (Some(h), _, _) if h.is_empty() => {
                return invalid("endpoint_host must not be empty");
            }
            _ => {}
        }
        let live = mode != Mode::Sim;
        if self.execution_order.is_some() != live || self.started_at.is_some() != live {
            return invalid(
                "execution_order and started_at are recorded in live and netem runs, and only there (CON-5(d), TRC-26)",
            );
        }
        self.build.check()?;
        if self.producers.is_empty() || self.producers.windows(2).any(|w| w[0] >= w[1]) {
            return invalid("producers must be a non-empty sorted list without duplicates");
        }
        let views = view_files()?;
        for (path, hash) in &self.files {
            Digest::from_hex(hash)?;
            if !layout_allows(path, &views) {
                return invalid(format!(
                    "`{path}` cannot be listed: a bundle holds the four tables, views/<view>.parquet and sidecar/ files, and nothing else (TRC-22)"
                ));
            }
        }
        let id = RunIdentity {
            seed,
            scenario_hash: Digest::from_hex(&self.scenario_hash)?,
            workload_hash: Digest::from_hex(&self.workload_hash)?,
            hypothesis_hash,
            engine_hash: Digest::from_hex(&self.engine_hash)?,
            mode,
            params_hash: identity::params_hash(&self.params)?,
        }
        .run_id()?;
        if id.to_hex() != self.run_id {
            return invalid(format!(
                "run_id {} is not the identity of the manifest's inputs ({id}) (CON-29)",
                self.run_id
            ));
        }
        Ok(id)
    }
}

/// The files of the derived views every bundle holds (TRC-22), as `views.toml`
/// names them: the one list the writer, the layout check and `verify` all read.
pub fn view_files() -> Result<Vec<String>> {
    Ok(schema::views()?.iter().map(|v| v.file.clone()).collect())
}

/// Every session of a bundle names the bundle's run (TRC-10).
fn check_sessions(trace: &Trace, run_id: &str) -> Result<()> {
    for s in trace.spans.iter().filter(|s| s.name == "acn.session") {
        match s.attrs.get("acn.run_id") {
            Some(AttrValue::String(r)) if r == run_id => {}
            other => {
                return invalid(format!(
                    "a session names run {other:?}, not the bundle's {run_id} (TRC-10)"
                ));
            }
        }
    }
    Ok(())
}

/// Whether a listed path belongs to the layout of TRC-22: one of the four tables,
/// a known view, or a file under `sidecar/`. The manifest itself and `logs/` are
/// never listed, and a verdict lives outside the bundle (HYP-20).
fn layout_allows(path: &str, views: &[String]) -> bool {
    if !clean_rel(path) {
        return false;
    }
    if [SPANS, EVENTS, LINKS, RESOURCES].contains(&path) || views.iter().any(|v| v == path) {
        return true;
    }
    path.strip_prefix("sidecar/")
        .is_some_and(|rest| !rest.is_empty())
}

/// A run seed must fit the signed 64-bit `acn.seed` attribute (TRC-10, CON-30),
/// although CON-27 writes it as an unsigned 64-bit integer; a larger seed is
/// refused rather than recorded wrongly (ADR-13).
pub fn check_seed(seed: u64) -> Result<()> {
    if i64::try_from(seed).is_err() {
        return invalid(format!(
            "seed {seed} does not fit the Int64 `acn.seed` attribute; run seeds are below 2^63 (ADR-13)"
        ));
    }
    Ok(())
}

/// The producers named by `resources`, sorted, after checking that each carries
/// the attributes of TRC-19 with the run's `engine_hash` and `build_hash`.
fn producers_of(
    resources: &[crate::model::ResourceRow],
    engine_hash: &str,
    build_hash: &str,
) -> Result<Vec<String>> {
    let mut producers = Vec::new();
    for r in resources {
        let s = |k: &str| match r.attrs.get(k) {
            Some(AttrValue::String(v)) if !v.is_empty() => Some(v.as_str()),
            _ => None,
        };
        let (Some(name), Some(_), Some(engine), Some(build)) = (
            s("service.name"),
            s("service.version"),
            s("acn.engine_hash"),
            s("acn.build_hash"),
        ) else {
            return invalid(format!(
                "resource {} lacks service.name, service.version, acn.engine_hash or acn.build_hash (TRC-19)",
                r.resource_id
            ));
        };
        if engine != engine_hash || build != build_hash {
            return invalid(format!(
                "resource `{name}` was produced under another engine or build than the run's (TRC-19)"
            ));
        }
        producers.push(name.to_owned());
    }
    producers.sort();
    producers.dedup();
    if producers.is_empty() {
        return invalid("a bundle has at least one producer");
    }
    Ok(producers)
}

/// A relative path with `/` separators and no empty, `.` or `..` component.
fn clean_rel(p: &str) -> bool {
    !p.is_empty()
        && !p.starts_with('/')
        && p.split('/').all(|c| !c.is_empty() && c != "." && c != "..")
}

/// The hypothesis a run uses, as the manifest records it.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct HypothesisRef {
    /// `[poc].id`, or `none`.
    pub id: String,
    /// The BLAKE3 of the hypothesis file, or [`Digest::ZERO`] for `none`.
    pub hash: Digest,
}

impl HypothesisRef {
    /// A run with no hypothesis.
    #[must_use]
    pub fn none() -> Self {
        Self {
            id: NO_HYPOTHESIS.into(),
            hash: Digest::ZERO,
        }
    }
}

/// Everything about a run that its bundle records and its `run_id` is derived from.
#[derive(Debug, Clone)]
pub struct RunSpec {
    pub seed: u64,
    pub mode: Mode,
    pub scenario_hash: Digest,
    pub workload_hash: Digest,
    pub hypothesis: HypothesisRef,
    pub params: RunParams,
    pub endpoint_host: Option<String>,
    pub execution_order: Option<Vec<String>>,
    pub started_at: Option<String>,
}

/// A bundle being written.
#[derive(Debug)]
pub struct Bundle {
    dir: PathBuf,
    manifest: Manifest,
    inv: schema::Inventory,
}

/// A bundle that has been written: the pair everything cites (TRC-23).
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct Written {
    pub dir: PathBuf,
    pub run_id: Digest,
    pub bundle_digest: Digest,
}

impl Bundle {
    /// Derive the `run_id` of `spec` and create `runs_dir/<run_id>/` with its `logs/`.
    /// Refuses an existing directory (CON-29) and a spec whose hypothesis is not the
    /// one the preflight checked (CON-28).
    pub fn create(
        runs_dir: &Path,
        preflight: &Preflight,
        build: &BuildInfo,
        spec: RunSpec,
    ) -> Result<Self> {
        let checked = match preflight.hypothesis() {
            RunHypothesis::None => spec.hypothesis.id == NO_HYPOTHESIS,
            h => {
                spec.hypothesis.id != NO_HYPOTHESIS
                    && h.status() == Some(spec.params.hyp_status)
                    && h.hash() == spec.hypothesis.hash
            }
        };
        if !checked {
            return Err(BundleError::Refused(
                "the run's hypothesis is not the one its preflight checked (CON-28)".into(),
            ));
        }
        check_seed(spec.seed)?;
        let inv = schema::inventory()?;
        let params = spec.params.pairs(&identity::options(&inv))?;
        let run_id = RunIdentity {
            seed: spec.seed,
            scenario_hash: spec.scenario_hash,
            workload_hash: spec.workload_hash,
            hypothesis_hash: spec.hypothesis.hash,
            engine_hash: preflight.engine_hash(),
            mode: spec.mode,
            params_hash: identity::params_hash(&params)?,
        }
        .run_id()?;
        let manifest = Manifest {
            run_id: run_id.to_hex(),
            seed: spec.seed.to_string(),
            mode: spec.mode.as_str().into(),
            backend: spec.params.backend.clone(),
            model: spec.params.model.clone(),
            endpoint_host: spec.endpoint_host,
            scenario_hash: spec.scenario_hash.to_hex(),
            workload_hash: spec.workload_hash.to_hex(),
            hypothesis: ManifestHypothesis {
                id: spec.hypothesis.id,
                status: spec.params.hyp_status.as_str().into(),
                hash: spec.hypothesis.hash.to_hex(),
            },
            engine_hash: preflight.engine_hash().to_hex(),
            build: build.clone(),
            params,
            execution_order: spec.execution_order,
            semconv_version: inv.semconv_version().to_owned(),
            // Filled from the resources by `finish`; a placeholder keeps the early
            // validation honest about everything else.
            producers: vec!["-".into()],
            started_at: spec.started_at,
            replicates: spec.params.replicates,
            files: BTreeMap::new(),
        };
        manifest.validate()?;

        std::fs::create_dir_all(runs_dir).map_err(io(runs_dir))?;
        let dir = runs_dir.join(run_id.to_hex());
        if let Err(e) = std::fs::create_dir(&dir) {
            return Err(if e.kind() == std::io::ErrorKind::AlreadyExists {
                BundleError::Refused(format!(
                    "{} exists; a bundle is never replaced (CON-29)",
                    dir.display()
                ))
            } else {
                BundleError::Io {
                    path: dir,
                    source: e,
                }
            });
        }
        let logs = dir.join(LOGS);
        std::fs::create_dir(&logs).map_err(io(&logs))?;
        Ok(Self { dir, manifest, inv })
    }

    /// The bundle directory.
    #[must_use]
    pub fn dir(&self) -> &Path {
        &self.dir
    }

    /// The `run_id`, known before anything is written.
    #[must_use]
    pub fn run_id(&self) -> &str {
        &self.manifest.run_id
    }

    /// `logs/`: stderr captures, never parsed and never hashed.
    #[must_use]
    pub fn logs_dir(&self) -> PathBuf {
        self.dir.join(LOGS)
    }

    /// Write the tables, then the manifest, last (TRC-23). Every resource must carry
    /// the attributes of TRC-19, with the run's `engine_hash` and `build_hash`.
    pub fn finish(mut self, trace: &Trace) -> Result<Written> {
        let producers = producers_of(
            &trace.resources,
            &self.manifest.engine_hash,
            &self.manifest.build.build_hash,
        )?;
        self.manifest.producers = producers;

        check_sessions(trace, &self.manifest.run_id)?;
        // The views are computed before anything is written, so a trace they
        // cannot be derived from leaves no tables behind either.
        let views = ingest::views(&self.inv, &schema::views()?, trace)?;
        parquet_io::write_trace(&self.dir, &self.inv, trace)?;
        for (view, batch) in &views {
            let path = self.dir.join(&view.file);
            if let Some(parent) = path.parent() {
                std::fs::create_dir_all(parent).map_err(io(parent))?;
            }
            parquet_io::write_batch(&path, batch)?;
        }
        self.manifest.files = hash_files(&self.dir)?;
        self.manifest.validate()?;
        let bytes = self.manifest.to_bytes()?;
        let path = self.dir.join(MANIFEST);
        write_new(&path, &bytes)?;
        Ok(Written {
            run_id: Digest::from_hex(&self.manifest.run_id)?,
            bundle_digest: Digest::of(&bytes),
            dir: self.dir,
        })
    }
}

fn write_new(path: &Path, bytes: &[u8]) -> Result<()> {
    use std::io::Write as _;
    let mut f = std::fs::OpenOptions::new()
        .write(true)
        .create_new(true)
        .open(path)
        .map_err(io(path))?;
    f.write_all(bytes).map_err(io(path))?;
    f.sync_all().map_err(io(path))
}

/// The BLAKE3 of every file in the bundle except the manifest and `logs/`. A
/// symbolic link or special file is an error.
fn hash_files(dir: &Path) -> Result<BTreeMap<String, String>> {
    let mut files = BTreeMap::new();
    let mut it = WalkDir::new(dir).sort_by_file_name().into_iter();
    while let Some(entry) = it.next() {
        let entry = entry?;
        let rel = crate::env::rel_path(dir, entry.path())
            .map_err(|e| BundleError::Invalid(e.to_string()))?;
        let ty = entry.file_type();
        if ty.is_dir() {
            if entry.depth() == 1 && rel == LOGS {
                it.skip_current_dir();
            }
            continue;
        }
        if !ty.is_file() {
            return invalid(format!(
                "{}: a bundle holds regular files only",
                entry.path().display()
            ));
        }
        if rel == MANIFEST {
            continue;
        }
        files.insert(rel, identity::file_hash(entry.path())?.to_hex());
    }
    Ok(files)
}

/// What [`verify`] established.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct Verified {
    pub run_id: Digest,
    pub bundle_digest: Digest,
    pub files: usize,
}

/// `acn bundle verify` (TRC-23): parse the manifest strictly, require its canonical
/// form, recompute `run_id` and `build_hash`, and recompute the hash of every listed
/// file; fail on any mismatch, on a missing table, and on any file that is neither
/// listed nor under `logs/`.
pub fn verify(dir: &Path) -> Result<Verified> {
    let path = dir.join(MANIFEST);
    let bytes = std::fs::read(&path).map_err(io(&path))?;
    let manifest: Manifest = serde_json::from_slice(&bytes)
        .map_err(|e| BundleError::Invalid(format!("{MANIFEST}: {e}")))?;
    if manifest.to_bytes()? != bytes {
        return invalid(format!(
            "{MANIFEST} is not in canonical form (sorted keys, no whitespace, one trailing newline; TRC-23)"
        ));
    }
    let run_id = manifest.validate()?;
    let name = dir.file_name().and_then(|n| n.to_str()).unwrap_or("");
    if name != manifest.run_id {
        return invalid(format!(
            "the bundle directory is `{name}`, not its run_id {} (TRC-22)",
            manifest.run_id
        ));
    }
    for table in [SPANS, EVENTS, LINKS, RESOURCES]
        .into_iter()
        .map(str::to_owned)
        .chain(view_files()?)
    {
        if !manifest.files.contains_key(&table) {
            return invalid(format!("{table} is not listed (TRC-22)"));
        }
    }
    let actual = hash_files(dir)?;
    for (p, h) in &manifest.files {
        match actual.get(p) {
            None => return invalid(format!("{p} is listed but missing (TRC-23)")),
            Some(a) if a != h => {
                return invalid(format!("{p} does not match its listed hash (TRC-23)"));
            }
            Some(_) => {}
        }
    }
    if let Some(extra) = actual.keys().find(|p| !manifest.files.contains_key(*p)) {
        return invalid(format!(
            "{extra} is neither listed nor under logs/ (TRC-23)"
        ));
    }
    // The hashes bind the files to the manifest; the resources must also say what
    // the manifest says, so that a replaced table cannot carry another engine,
    // build or producer list (TRC-19).
    let resources = parquet_io::read_resources(&dir.join(RESOURCES))?;
    let producers = producers_of(
        &resources,
        &manifest.engine_hash,
        &manifest.build.build_hash,
    )?;
    if producers != manifest.producers {
        return invalid(format!(
            "the manifest names producers {:?}, the resources {producers:?} (TRC-19)",
            manifest.producers
        ));
    }
    Ok(Verified {
        run_id,
        bundle_digest: Digest::of(&bytes),
        files: manifest.files.len(),
    })
}

/// `acn bundle verify --views` (TRC-35): everything [`verify`] checks, then read the
/// four tables back and require them to be exactly what the writer makes of what
/// was read (so the promoted columns agree with `attrs`, which the views do not
/// read), recompute the five views from them alone, and require each view file to
/// be byte-identical to the recomputation.
pub fn verify_views(dir: &Path) -> Result<Verified> {
    let verified = verify(dir)?;
    let inv = schema::inventory()?;
    let trace = parquet_io::read_trace(dir, &inv)?;
    let tables = parquet_io::batches(&inv, &trace)?;
    for (file, batch) in [SPANS, EVENTS, LINKS, RESOURCES].into_iter().zip(&tables) {
        let path = dir.join(file);
        let on_disk = std::fs::read(&path).map_err(io(&path))?;
        if parquet_io::encode(batch)? != on_disk {
            return invalid(format!(
                "{file} is not the canonical encoding of its own rows: a promoted column disagrees with `attrs`, or the file was not written by this writer (TRC-25, TRC-35)"
            ));
        }
    }
    check_sessions(&trace, &verified.run_id.to_hex())?;
    for (view, batch) in ingest::views(&inv, &schema::views()?, &trace)? {
        let path = dir.join(&view.file);
        let on_disk = std::fs::read(&path).map_err(io(&path))?;
        if parquet_io::encode(&batch)? != on_disk {
            return invalid(format!(
                "{} differs from the view recomputed from the tables (TRC-35)",
                view.file
            ));
        }
    }
    Ok(verified)
}
