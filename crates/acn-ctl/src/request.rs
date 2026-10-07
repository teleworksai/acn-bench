//! A run request (SPEC 070 CTL-10): exactly the inputs a CLI run takes, as
//! JSON, with unknown fields refused.

use std::collections::BTreeMap;

use serde::{Deserialize, Serialize};

use crate::Refusal;

/// What runs the request: the agent loop (HAR-50) or the generator (GEN-22).
#[derive(Debug, Clone, Copy, PartialEq, Eq, Serialize, Deserialize)]
#[serde(rename_all = "lowercase")]
pub enum Kind {
    Harness,
    Generator,
}

/// The `opt.` options of HAR-25 that have CLI flags, by their names without
/// the prefix; an absent one takes its default. `endpoint_name` names a
/// registered endpoint (CTL-30) in place of `endpoint`.
#[derive(Debug, Clone, Default, PartialEq, Serialize, Deserialize)]
#[serde(deny_unknown_fields)]
pub struct ReqOpts {
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub endpoint: Option<String>,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub endpoint_name: Option<String>,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub max_retries: Option<u64>,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub retry_base_ms: Option<u64>,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub request_timeout_ms: Option<u64>,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub stall_threshold_ms: Option<f64>,
}

/// A scenario named by its workspace path.
#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
#[serde(deny_unknown_fields)]
pub struct ByPath {
    pub path: String,
}

/// A stored scenario named by its hash (CTL-20).
#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
#[serde(deny_unknown_fields)]
pub struct ByHash {
    pub hash: String,
}

/// A request's scenario (CTL-10).
#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
#[serde(untagged)]
pub enum ScenarioRef {
    Path(ByPath),
    Hash(ByHash),
}

/// A run request as submitted (CTL-10).
#[derive(Debug, Clone, PartialEq, Serialize, Deserialize)]
#[serde(deny_unknown_fields)]
pub struct Submit {
    pub kind: Kind,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub workload: Option<String>,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub backend: Option<String>,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub model: Option<String>,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub sheet: Option<String>,
    pub mode: String,
    pub arm: String,
    pub replicates: u32,
    /// Parameter name to its value as text, typed later as the CLI types it.
    #[serde(default)]
    pub vary: BTreeMap<String, String>,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub hypothesis: Option<String>,
    /// A run seed, as a decimal string (JSON numbers lose precision above
    /// 2^53), for hypothesis `none` only (HYP-9).
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub seed: Option<String>,
    #[serde(default)]
    pub opt: ReqOpts,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub scenario: Option<ScenarioRef>,
    /// Run a failed request again (CTL-13); not part of its identity.
    #[serde(default, skip_serializing)]
    pub retry: bool,
}

impl Submit {
    /// Parse a request body (CTL-2: unknown fields are refused).
    pub fn parse(body: &[u8]) -> Result<Self, Refusal> {
        serde_json::from_slice(body).map_err(|e| Refusal::bad("bad_request", e.to_string()))
    }

    /// The checks of CTL-10 that need no file.
    pub fn validate(&self) -> Result<(), Refusal> {
        let bad = |m: &str| Err(Refusal::bad("bad_request", format!("{m} (CTL-10)")));
        match self.kind {
            Kind::Harness => {
                if self.workload.is_none() || self.backend.is_none() || self.model.is_none() {
                    return bad("a harness request names its workload, backend and model");
                }
                if self.sheet.is_some() {
                    return bad("a harness request has no sheet");
                }
            }
            Kind::Generator => {
                if self.sheet.is_none() {
                    return bad("a generator request names its sheet");
                }
                if self.workload.is_some() || self.backend.is_some() || self.model.is_some() {
                    return bad(
                        "a generator request has no workload, backend or model: its sheet names them",
                    );
                }
            }
        }
        match (&self.hypothesis, &self.seed) {
            (Some(_), None) => {}
            (None, Some(s)) => {
                let ok = !s.is_empty()
                    && s.bytes().all(|b| b.is_ascii_digit())
                    && s.parse::<u64>().is_ok_and(|v| v <= i64::MAX as u64);
                if !ok {
                    return bad("`seed` is a decimal string from 0 to 2^63 - 1");
                }
            }
            _ => return bad("give exactly one of `hypothesis` and `seed`"),
        }
        if acn_trace::identity::Mode::parse(&self.mode).is_err() {
            return bad("`mode` is sim or live");
        }
        if !matches!(self.arm.as_str(), "treatment" | "control") {
            return bad("`arm` is treatment or control");
        }
        if self.replicates == 0 {
            return bad("`replicates` must be positive");
        }
        if let Some(b) = &self.backend
            && acn_harness::wire::Backend::parse(b).is_err()
        {
            return bad("`backend` is not one of HAR-20's");
        }
        if self.opt.endpoint.is_some() && self.opt.endpoint_name.is_some() {
            return bad("give `endpoint` or `endpoint_name`, not both");
        }
        if let Some(f) = self.opt.stall_threshold_ms
            && !f.is_finite()
        {
            return bad("`stall_threshold_ms` must be finite");
        }
        Ok(())
    }
}
