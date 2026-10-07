//! Resolving a request (SPEC 070 CTL-10, CTL-11): its paths inside the
//! workspace, every input file's BLAKE3, its endpoint's URL, and the
//! canonical JSON its `request_id` is the hash of.

use std::collections::BTreeMap;
use std::path::{Component, Path, PathBuf};

use serde::{Deserialize, Serialize};
use serde_json::Value;

use crate::Refusal;
use crate::request::{ByPath, Kind, ScenarioRef, Submit};

/// The context string of a `request_id` (CON-27).
pub const REQUEST_CONTEXT: &[u8] = b"acn-bench/ctl_request/v1\0";

/// A request as submitted, with its paths normalised to the workspace root,
/// every input file's hash by path, and its endpoint's URL (CTL-11).
#[derive(Debug, Clone, PartialEq, Serialize, Deserialize)]
#[serde(deny_unknown_fields)]
pub struct Resolved {
    pub request: Submit,
    pub hashes: BTreeMap<String, String>,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub endpoint_url: Option<String>,
}

/// A registered endpoint (CTL-30).
#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
#[serde(deny_unknown_fields)]
pub struct Endpoint {
    pub backend: String,
    pub url: String,
}

/// `p`, relative to the canonical workspace `root`, resolved inside it after
/// following links, with no `..` (CTL-10); its canonical path relative to
/// the root, with `/` separators.
pub fn inside(root: &Path, p: &str) -> Result<(PathBuf, String), Refusal> {
    let refuse = |m: String| Err(Refusal::bad("path_refused", format!("{m} (CTL-10)")));
    let rel = Path::new(p);
    if p.is_empty()
        || rel.is_absolute()
        || rel
            .components()
            .any(|c| !matches!(c, Component::Normal(_) | Component::CurDir))
    {
        return refuse(format!(
            "`{p}` is not a path relative to the workspace root"
        ));
    }
    let abs = match std::fs::canonicalize(root.join(rel)) {
        Ok(a) => a,
        Err(e) => return refuse(format!("`{p}`: {e}")),
    };
    let Ok(under) = abs.strip_prefix(root) else {
        return refuse(format!("`{p}` resolves outside the workspace"));
    };
    let text = under
        .components()
        .map(|c| c.as_os_str().to_string_lossy().into_owned())
        .collect::<Vec<_>>()
        .join("/");
    Ok((abs, text))
}

fn hash_file(abs: &Path) -> Result<String, Refusal> {
    acn_trace::identity::file_hash(abs)
        .map(|d| d.to_hex())
        .map_err(|e| Refusal::bad("path_refused", e.to_string()))
}

/// The directory of a stored scenario (CTL-20).
#[must_use]
pub fn scenario_dir(ctl: &Path, hash: &str) -> PathBuf {
    ctl.join("scenarios").join(hash)
}

/// A stored scenario's file: the one TOML in its directory, whose bytes
/// still have their hash (CTL-20).
pub fn stored_scenario(ctl: &Path, hash: &str) -> Result<PathBuf, Refusal> {
    let unknown = || {
        Refusal::new(
            404,
            "unknown_scenario",
            format!("no stored scenario {hash} (CTL-20)"),
        )
    };
    if !is_hex64(hash) {
        return Err(unknown());
    }
    let dir = scenario_dir(ctl, hash);
    let rd = std::fs::read_dir(&dir).map_err(|_| unknown())?;
    let mut tomls: Vec<PathBuf> = rd
        .filter_map(Result::ok)
        .map(|e| e.path())
        .filter(|p| p.extension().is_some_and(|x| x == "toml"))
        .collect();
    tomls.sort();
    match tomls.as_slice() {
        [one] if hash_file(one).ok().as_deref() == Some(hash) => Ok(one.clone()),
        [_] => Err(Refusal::new(
            409,
            "input_changed",
            format!("stored scenario {hash} no longer has its hash (CTL-20)"),
        )),
        _ => Err(unknown()),
    }
}

/// A registered endpoint, by name (CTL-30).
pub fn endpoint(ctl: &Path, name: &str) -> Result<Endpoint, Refusal> {
    let unknown = || {
        Refusal::new(
            404,
            "unknown_endpoint",
            format!("no endpoint `{name}` (CTL-30)"),
        )
    };
    if !is_endpoint_name(name) {
        return Err(unknown());
    }
    let text =
        std::fs::read(ctl.join("endpoints").join(format!("{name}.json"))).map_err(|_| unknown())?;
    serde_json::from_slice(&text).map_err(Refusal::internal)
}

/// 64 lowercase hex digits: a run_id, request_id or scenario hash (CTL-3).
#[must_use]
pub fn is_hex64(s: &str) -> bool {
    s.len() == 64
        && s.bytes()
            .all(|b| b.is_ascii_digit() || (b'a'..=b'f').contains(&b))
}

/// 1 to 64 lowercase ASCII letters, digits and `-` (CTL-30).
#[must_use]
pub fn is_endpoint_name(s: &str) -> bool {
    (1..=64).contains(&s.len())
        && s.bytes()
            .all(|b| b.is_ascii_lowercase() || b.is_ascii_digit() || b == b'-')
}

/// Resolve `r` against the workspace `root` and the registry `ctl` (CTL-10,
/// CTL-11): every path normalised, every input file hashed, the endpoint's
/// URL looked up, and its backend checked.
pub fn resolve(root: &Path, ctl: &Path, r: &Submit) -> Result<Resolved, Refusal> {
    r.validate()?;
    let mut request = r.clone();
    request.retry = false;
    // CON-27(c): −0 is written as 0.
    if let Some(f) = request.opt.stall_threshold_ms
        && f == 0.0
    {
        request.opt.stall_threshold_ms = Some(0.0);
    }
    let mut hashes = BTreeMap::new();
    let take = |p: &mut String, hashes: &mut BTreeMap<String, String>| -> Result<(), Refusal> {
        let (abs, rel) = inside(root, p)?;
        hashes.insert(rel.clone(), hash_file(&abs)?);
        *p = rel;
        Ok(())
    };
    for p in [
        &mut request.workload,
        &mut request.sheet,
        &mut request.hypothesis,
    ]
    .into_iter()
    .flatten()
    {
        take(p, &mut hashes)?;
    }
    match &mut request.scenario {
        Some(ScenarioRef::Path(ByPath { path })) => take(path, &mut hashes)?,
        Some(ScenarioRef::Hash(h)) => {
            stored_scenario(ctl, &h.hash)?;
        }
        None => {}
    }
    let backend = match request.kind {
        Kind::Harness => request.backend.clone().unwrap_or_default(),
        Kind::Generator => "mockllm".into(),
    };
    let endpoint_url = match &request.opt.endpoint_name {
        Some(name) => {
            let e = endpoint(ctl, name)?;
            if e.backend != backend {
                return Err(Refusal::bad(
                    "backend_mismatch",
                    format!(
                        "endpoint `{name}` is a `{}` endpoint, not `{backend}` (CTL-30)",
                        e.backend
                    ),
                ));
            }
            Some(e.url)
        }
        None => None,
    };
    Ok(Resolved {
        request,
        hashes,
        endpoint_url,
    })
}

/// JSON with object keys sorted and no insignificant whitespace (CTL-11):
/// rendered here, so that no serializer setting can change it.
fn canonical(v: &Value) -> String {
    match v {
        Value::Object(m) => {
            let mut keys: Vec<&String> = m.keys().collect();
            keys.sort();
            let fields: Vec<String> = keys
                .into_iter()
                .map(|k| {
                    format!(
                        "{}:{}",
                        serde_json::to_string(k).unwrap_or_default(),
                        m.get(k).map(canonical).unwrap_or_default()
                    )
                })
                .collect();
            format!("{{{}}}", fields.join(","))
        }
        Value::Array(a) => format!(
            "[{}]",
            a.iter().map(canonical).collect::<Vec<_>>().join(",")
        ),
        other => other.to_string(),
    }
}

impl Resolved {
    /// `request.json`'s bytes: the canonical JSON and one newline (CTL-11).
    pub fn text(&self) -> Result<String, Refusal> {
        let v = serde_json::to_value(self).map_err(Refusal::internal)?;
        Ok(format!("{}\n", canonical(&v)))
    }

    /// `request_id`: the hex BLAKE3 of the context string and `request.json`'s
    /// bytes (CTL-11).
    pub fn request_id(&self) -> Result<String, Refusal> {
        let mut h = blake3::Hasher::new();
        h.update(REQUEST_CONTEXT);
        h.update(self.text()?.as_bytes());
        Ok(h.finalize().to_hex().to_string())
    }

    /// The inputs whose bytes no longer have their recorded hash (CTL-12).
    #[must_use]
    pub fn changed(&self, root: &Path) -> Vec<String> {
        self.hashes
            .iter()
            .filter(|(rel, h)| {
                acn_trace::identity::file_hash(&root.join(rel.as_str()))
                    .map(|d| d.to_hex())
                    .ok()
                    .as_deref()
                    != Some(h.as_str())
            })
            .map(|(rel, _)| rel.clone())
            .collect()
    }
}
