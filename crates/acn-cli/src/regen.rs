//! `acn run --from-run-id` (SPEC 140 P16-2 to P16-6): regenerate a `sim`
//! bundle from its `run_id`, finding every input by its hash, into a scratch
//! directory, and compare it with the original.

use std::collections::BTreeMap;
use std::path::{Path, PathBuf};

use acn_harness::agent::Opts;
use acn_harness::run::{HypothesisArg, RunConfig};
use acn_harness::wire::Backend;
use acn_trace::bundle::{self, MANIFEST, Manifest};
use acn_trace::identity::{BuildInfo, Digest, Mode};
use serde_json::{Value, json};

/// What `acn run --from-run-id` is told.
pub struct Regen<'a> {
    pub run_id: &'a str,
    /// Resolved against the workspace root (CON-28), or `start_dir` without one.
    pub runs_dir: &'a Path,
    pub across_builds: bool,
    pub start_dir: &'a Path,
    pub engine_hash: Digest,
    pub build: BuildInfo,
}

/// A refusal: its code (P16-6), what went wrong, and the hash it is about.
struct Refusal {
    code: &'static str,
    error: String,
    hash: Option<String>,
}

fn refuse(code: &'static str, error: impl Into<String>) -> Refusal {
    Refusal {
        code,
        error: error.into(),
        hash: None,
    }
}

type R<T> = Result<T, Refusal>;

/// The bundle file that records the build in every row (TRC-19).
const RESOURCES: &str = "resources.parquet";

const ZERO: &str = "0000000000000000000000000000000000000000000000000000000000000000";

/// The directories inputs are found in, relative to the base (P16-3).
const INPUT_ROOTS: [&str; 4] = ["workloads", "scenarios", "lab", "kit/inputs"];
const HYPOTHESIS_ROOTS: [&str; 3] = ["hypotheses", "lab/hypotheses", "kit/inputs"];

/// `acn run --from-run-id` (P16-2): one object (CON-8).
#[must_use]
pub fn regenerate(r: &Regen<'_>) -> Value {
    match inner(r) {
        Ok(v) => v,
        Err(e) => {
            let mut o = json!({"ok": false, "code": e.code, "error": e.error, "run_id": r.run_id});
            if let Some(h) = e.hash {
                o["hash"] = h.into();
            }
            o
        }
    }
}

/// Every `*.toml` file under `dir`, directories named `target` skipped, in
/// bytewise path order.
fn tomls(dir: &Path, out: &mut Vec<PathBuf>) {
    let Ok(rd) = std::fs::read_dir(dir) else {
        return;
    };
    let mut entries: Vec<_> = rd.filter_map(Result::ok).collect();
    entries.sort_by_key(std::fs::DirEntry::file_name);
    for e in entries {
        let Ok(t) = e.file_type() else { continue };
        let p = e.path();
        if t.is_dir() {
            if e.file_name() != "target" {
                tomls(&p, out);
            }
        } else if t.is_file() && p.extension().is_some_and(|x| x == "toml") {
            out.push(p);
        }
    }
}

/// Every `*.toml` file under `roots`, in bytewise path order (P16-3), with
/// its BLAKE3. A file that cannot be read is skipped: it is no input.
fn candidates(base: &Path, roots: &[PathBuf]) -> Vec<(PathBuf, String)> {
    let mut all = Vec::new();
    for r in roots {
        tomls(&base.join(r), &mut all);
    }
    all.sort_by(|a, b| {
        a.as_os_str()
            .as_encoded_bytes()
            .cmp(b.as_os_str().as_encoded_bytes())
    });
    all.dedup();
    all.into_iter()
        .filter_map(|p| {
            let h = std::fs::read(&p).ok()?;
            Some((p, blake3::hash(&h).to_hex().to_string()))
        })
        .collect()
}

/// The first candidate with BLAKE3 `hash` that `keep` accepts.
fn find(all: &[(PathBuf, String)], hash: &str, keep: &dyn Fn(&Path) -> bool) -> Option<PathBuf> {
    all.iter()
        .find(|(p, h)| h == hash && keep(p))
        .map(|(p, _)| p.clone())
}

fn missing(what: &str, hash: &str) -> Refusal {
    Refusal {
        code: "input_missing",
        error: format!("no {what} with BLAKE3 {hash} under the search roots (P16-3)"),
        hash: Some(hash.to_owned()),
    }
}

/// The smallest positive `n` whose directory under `parent` this call creates:
/// one that exists, even one made by a concurrent regeneration, is never taken.
fn fresh(parent: &Path) -> R<PathBuf> {
    std::fs::create_dir_all(parent)
        .map_err(|e| refuse("io", format!("{}: {e}", parent.display())))?;
    for n in 1u64.. {
        let d = parent.join(n.to_string());
        match std::fs::create_dir(&d) {
            Ok(()) => return Ok(d),
            Err(e) if e.kind() == std::io::ErrorKind::AlreadyExists => {}
            Err(e) => return Err(refuse("io", format!("{}: {e}", d.display()))),
        }
    }
    Err(refuse("io", "no free regeneration directory"))
}

/// The manifest as JSON, without what records its build (P16-6).
fn neutral_manifest(m: &Manifest) -> R<Value> {
    let mut v = serde_json::to_value(m).map_err(|e| refuse("io", e.to_string()))?;
    if let Some(o) = v.as_object_mut() {
        o.remove("build");
        // The file hashes are compared on their own (P16-6).
        o.remove("files");
    }
    Ok(v)
}

/// The resources without each row's `acn.build_hash` (TRC-19).
fn neutral_resources(dir: &Path) -> R<Vec<acn_trace::model::ResourceRow>> {
    let mut rows = acn_trace::parquet_io::read_resources(&dir.join(RESOURCES))
        .map_err(|e| refuse("bundle_invalid", e.to_string()))?;
    for r in &mut rows {
        r.attrs.remove("acn.build_hash");
    }
    Ok(rows)
}

fn inner(r: &Regen<'_>) -> R<Value> {
    if r.run_id.len() != 64
        || !r
            .run_id
            .bytes()
            .all(|b| b.is_ascii_digit() || (b'a'..=b'f').contains(&b))
    {
        return Err(refuse(
            "unknown_run",
            format!("`{}` is not a run_id: 64 lowercase hex (CON-29)", r.run_id),
        ));
    }
    let start = std::fs::canonicalize(r.start_dir)
        .map_err(|e| refuse("io", format!("{}: {e}", r.start_dir.display())))?;
    let base = acn_trace::env::find_root(&start)
        .map_err(|e| refuse("io", e.to_string()))?
        .unwrap_or(start);
    let runs = if r.runs_dir.is_absolute() {
        r.runs_dir.to_path_buf()
    } else {
        base.join(r.runs_dir)
    };
    // P16-2: the original manifest, from the bundle when it exists.
    let original = runs.join(r.run_id);
    let (manifest, have_bundle) = if original.exists() {
        let v = bundle::verify(&original)
            .map_err(|e| refuse("bundle_invalid", format!("{}: {e}", original.display())))?;
        (v.manifest, true)
    } else {
        let path = base
            .join("kit/manifests")
            .join(format!("{}.json", r.run_id));
        let bytes = std::fs::read(&path).map_err(|_| {
            refuse(
                "unknown_run",
                format!(
                    "neither {} nor {} exists (P16-2)",
                    original.display(),
                    path.display()
                ),
            )
        })?;
        let m: Manifest = serde_json::from_slice(&bytes)
            .map_err(|e| refuse("manifest_invalid", format!("{}: {e}", path.display())))?;
        if m.to_bytes()
            .map_err(|e| refuse("manifest_invalid", e.to_string()))?
            != bytes
        {
            return Err(refuse(
                "manifest_invalid",
                format!("{} is not in canonical form (TRC-23)", path.display()),
            ));
        }
        (m, false)
    };
    let recomputed = manifest
        .validate()
        .map_err(|e| refuse("manifest_invalid", e.to_string()))?;
    if recomputed.to_hex() != r.run_id || manifest.run_id != r.run_id {
        return Err(refuse(
            "manifest_invalid",
            format!(
                "the manifest's run_id does not recompute to {} (CON-29)",
                r.run_id
            ),
        ));
    }
    if manifest.mode != "sim" {
        return Err(refuse(
            "not_reproducible_mode",
            format!(
                "a `{}` bundle reproduces statistically, not byte for byte (CON-5(c))",
                manifest.mode
            ),
        ));
    }
    let same_build = manifest.build.build_hash == r.build.build_hash;
    if !same_build && !r.across_builds {
        return Err(refuse(
            "not_regenerable_with_this_build",
            format!(
                "the bundle was made by build {}, this binary is {} (CON-31); --across-builds compares without claiming identity",
                manifest.build.build_hash, r.build.build_hash
            ),
        ));
    }
    // P16-4: which run.
    let producers = &manifest.producers;
    let generator = producers.iter().any(|p| p == "acn-gen");
    if !generator && !producers.iter().any(|p| p == "acn-harness") {
        return Err(refuse(
            "unknown_producer",
            format!("producers {producers:?} name neither acn-harness nor acn-gen (P16-4)"),
        ));
    }
    // P16-3: the inputs, by hash.
    let roots: Vec<PathBuf> = INPUT_ROOTS
        .iter()
        .map(PathBuf::from)
        .chain(std::iter::once(runs.join("ctl/scenarios")))
        .collect();
    let any = |_: &Path| true;
    let inputs = candidates(&base, &roots);
    let workload = find(&inputs, &manifest.workload_hash, &any).ok_or_else(|| {
        missing(
            if generator { "sheet" } else { "workload" },
            &manifest.workload_hash,
        )
    })?;
    let scenario = if manifest.scenario_hash == ZERO {
        None
    } else {
        let s = find(&inputs, &manifest.scenario_hash, &any)
            .ok_or_else(|| missing("scenario", &manifest.scenario_hash))?;
        Some(s)
    };
    let hypothesis = if manifest.hypothesis.hash == ZERO {
        let seed = manifest
            .seed
            .parse::<u64>()
            .map_err(|_| refuse("manifest_invalid", "the seed is not a decimal (CON-30)"))?;
        HypothesisArg::None { seed }
    } else {
        // Status follows location, as the harness derives it (HYP-3).
        let frozen_dir = base.join("hypotheses");
        let want_frozen = manifest.hypothesis.status == "frozen";
        let keep = |p: &Path| p.starts_with(&frozen_dir) == want_frozen;
        let hroots: Vec<PathBuf> = HYPOTHESIS_ROOTS.iter().map(PathBuf::from).collect();
        let h = find(
            &candidates(&base, &hroots),
            &manifest.hypothesis.hash,
            &keep,
        )
        .ok_or_else(|| {
            missing(
                &format!("{} hypothesis", manifest.hypothesis.status),
                &manifest.hypothesis.hash,
            )
        })?;
        HypothesisArg::File(h)
    };
    // P16-4: the recorded fields.
    let p = &manifest.params;
    let arm = p
        .get("arms")
        .filter(|a| !a.contains(','))
        .cloned()
        .ok_or_else(|| refuse("manifest_invalid", "a bundle records one arm (HAR-50)"))?;
    let mut vary = BTreeMap::new();
    let mut opts = Opts::default();
    let int = |k: &str, v: &str| -> R<u64> {
        let x: f64 = v
            .parse()
            .map_err(|_| refuse("manifest_invalid", format!("`{k}` = `{v}` is not a number")))?;
        if x < 0.0 || x.fract() != 0.0 || x > 1e15 {
            return Err(refuse(
                "manifest_invalid",
                format!("`{k}` = `{v}` is not a whole number"),
            ));
        }
        #[allow(clippy::cast_possible_truncation, clippy::cast_sign_loss)]
        Ok(x as u64)
    };
    // Parameter keys are `<namespace>.<name>` (CON-29): the varied ones and
    // the run options are read back; the rest are identity only.
    for (k, v) in p {
        let Some((ns, name)) = k.split_once('.') else {
            continue;
        };
        if ns == "vary" {
            vary.insert(name.to_owned(), v.clone());
        } else if ns == "opt" {
            match name {
                "endpoint" => opts.endpoint = v.clone(),
                "max_retries" => opts.max_retries = int(k, v)?,
                "retry_base_ms" => opts.retry_base_ms = int(k, v)?,
                "request_timeout_ms" => opts.request_timeout_ms = int(k, v)?,
                "stall_threshold_ms" => {
                    opts.stall_threshold_ms = v
                        .parse::<f64>()
                        .ok()
                        .filter(|x| x.is_finite() && *x >= 0.0)
                        .ok_or_else(|| {
                            refuse(
                                "manifest_invalid",
                                format!("`{k}` = `{v}` is not a finite, non-negative number"),
                            )
                        })?;
                }
                other => {
                    return Err(refuse(
                        "manifest_invalid",
                        format!("`opt.{other}` cannot be set by a run (CON-29)"),
                    ));
                }
            }
        }
    }
    let mode =
        Mode::parse(&manifest.mode).map_err(|e| refuse("manifest_invalid", e.to_string()))?;
    // P16-4: a generator's backend and model are the sheet's, checked here.
    if generator {
        let sheet = acn_gen::sheet::Sheet::load(&workload)
            .map_err(|e| refuse("input_missing", format!("{}: {e}", workload.display())))?;
        if manifest.backend != "mockllm" || sheet.model != manifest.model {
            return Err(refuse(
                "manifest_invalid",
                format!(
                    "the sheet names model `{}` on the mock; the manifest records `{}` on `{}` (P16-4)",
                    sheet.model, manifest.model, manifest.backend
                ),
            ));
        }
    }
    // P16-6: a fresh scratch directory.
    let dir = fresh(&runs.join("regen").join(r.run_id))?;
    let written = if generator {
        let cfg = acn_gen::run::GenConfig {
            sheet: workload,
            mode,
            arm,
            replicates: manifest.replicates,
            vary,
            opts,
            hypothesis,
            runs_dir: dir.clone(),
            start_dir: base.clone(),
            engine_hash: r.engine_hash,
            build: r.build.clone(),
            profiles: None,
        };
        acn_gen::run::run(&cfg, scenario.as_deref()).map(|w| w.written)
    } else {
        let backend = Backend::parse(&manifest.backend)
            .map_err(|e| refuse("manifest_invalid", e.to_string()))?;
        let cfg = RunConfig {
            workload,
            backend,
            model: manifest.model.clone(),
            mode,
            arm,
            replicates: manifest.replicates,
            vary,
            opts,
            hypothesis,
            runs_dir: dir.clone(),
            start_dir: base.clone(),
            engine_hash: r.engine_hash,
            build: r.build.clone(),
            profiles: None,
        };
        acn_harness::run::run_with_scenario(&cfg, scenario.as_deref())
    }
    .map_err(|e| refuse("run_failed", e.to_string()))?;
    if written.run_id.to_hex() != r.run_id {
        return Err(refuse(
            "run_id_differs",
            format!(
                "the inputs found make run {}, not {} (P16-4)",
                written.run_id.to_hex(),
                r.run_id
            ),
        ));
    }
    let new = bundle::verify(&written.dir).map_err(|e| refuse("bundle_invalid", e.to_string()))?;
    // The scenario span's text needs no separate check (P16-3): the file found
    // has `scenario_hash`, the original's span text has it too (EMU-37), and on
    // the same build the spans are compared byte for byte below.
    let mut differ: Vec<String> = Vec::new();
    if same_build {
        let a = manifest
            .to_bytes()
            .map_err(|e| refuse("io", e.to_string()))?;
        let b = new
            .manifest
            .to_bytes()
            .map_err(|e| refuse("io", e.to_string()))?;
        if a != b {
            for (path, h) in &manifest.files {
                if new.manifest.files.get(path) != Some(h) {
                    differ.push(path.clone());
                }
            }
            for path in new.manifest.files.keys() {
                if !manifest.files.contains_key(path) {
                    differ.push(path.clone());
                }
            }
            differ.push(MANIFEST.to_owned());
        }
    } else {
        for (path, h) in &manifest.files {
            if path != RESOURCES && new.manifest.files.get(path) != Some(h) {
                differ.push(path.clone());
            }
        }
        for path in new.manifest.files.keys() {
            if !manifest.files.contains_key(path) {
                differ.push(path.clone());
            }
        }
        if have_bundle && neutral_resources(&original)? != neutral_resources(&written.dir)? {
            differ.push(RESOURCES.to_owned());
        }
        if neutral_manifest(&manifest)? != neutral_manifest(&new.manifest)? {
            differ.push(MANIFEST.to_owned());
        }
    }
    differ.sort();
    differ.dedup();
    let identical = differ.is_empty();
    Ok(json!({
        "ok": if same_build { identical } else { true },
        "run_id": r.run_id,
        "dir": written.dir.display().to_string(),
        "same_build": same_build,
        "identical": identical,
        "differ": differ,
    }))
}

/// `acn bundle neutral <dir>` (P16-12): a verified bundle in build-neutral
/// form, for comparing targets. Its `files` are the listed hashes but
/// `resources.parquet`'s, its `resources` the decoded rows without
/// `acn.build_hash` (each written as its debug text, which is the same on
/// every target), and its `manifest` the manifest without `build`.
#[must_use]
pub fn neutral(dir: &Path) -> Value {
    let inner = || -> R<Value> {
        let v = bundle::verify(dir)
            .map_err(|e| refuse("bundle_invalid", format!("{}: {e}", dir.display())))?;
        let mut files: BTreeMap<&str, &str> = BTreeMap::new();
        for (p, h) in &v.manifest.files {
            if p != RESOURCES {
                files.insert(p, h);
            }
        }
        let resources: Vec<String> = neutral_resources(dir)?
            .iter()
            .map(|r| format!("{r:?}"))
            .collect();
        Ok(json!({
            "ok": true,
            "run_id": v.manifest.run_id,
            "target": v.manifest.build.target,
            "build_hash": v.manifest.build.build_hash,
            "files": files,
            "resources": resources,
            "manifest": neutral_manifest(&v.manifest)?,
        }))
    };
    inner().unwrap_or_else(|e| json!({"ok": false, "code": e.code, "error": e.error}))
}

#[cfg(test)]
mod tests {
    use super::*;

    /// Cites: P16-3
    #[test]
    fn candidates_are_in_bytewise_path_order_and_only_toml() {
        let d = tempfile::tempdir().unwrap();
        for sub in ["lab/x", "lab/x-y"] {
            std::fs::create_dir_all(d.path().join(sub)).unwrap();
            std::fs::write(d.path().join(sub).join("w.toml"), "same").unwrap();
        }
        std::fs::write(d.path().join("lab/x/w.json"), "same").unwrap();
        let all = candidates(d.path(), &[PathBuf::from("lab")]);
        let hash = blake3::hash(b"same").to_hex().to_string();
        // `-` (0x2d) sorts before `/` (0x2f): `lab/x-y/w.toml` comes first,
        // though `x` < `x-y` component by component.
        let got = find(&all, &hash, &|_| true).unwrap();
        assert!(got.ends_with("lab/x-y/w.toml"), "{}", got.display());
        assert_eq!(all.len(), 2, "only *.toml files are inputs");
    }
}
