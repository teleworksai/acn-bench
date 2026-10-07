//! The sheet (SPEC 050 GEN-1) and its distributions (GEN-2).

use std::collections::BTreeMap;
use std::path::Path;

use acn_harness::agent::below;
use rand_chacha::ChaCha20Rng;
use serde::Deserialize;

use crate::GenError;

/// SPEC 010's tool classes (TRC-13), in the order tools are listed (GEN-11).
pub const CLASSES: [&str; 7] = [
    "file", "shell", "search", "http", "subagent", "testbed", "other",
];

/// The class a fan-out spawns with (GEN-12): never drawn by `tool_class`.
pub const SUBAGENT: &str = "subagent";

/// The parts per million a quantile table spans (GEN-2).
pub const PPM: u64 = 1_000_000;

/// The largest value a count may take: sessions, turns, sub-agents. A plan
/// holds them in memory, so a sheet beyond these is refused at load (ADR-38).
pub const MAX_COUNT: u64 = 1_000_000;

/// The largest number of tokens of one text: 2^24, 64 MiB of content.
pub const MAX_TOKENS: u64 = 1 << 24;

/// A distribution as written: exactly one key (GEN-2).
#[derive(Debug, Clone, Deserialize)]
#[serde(deny_unknown_fields)]
struct RawDist {
    #[serde(rename = "const")]
    constant: Option<u64>,
    uniform: Option<[u64; 2]>,
    quantiles: Option<Vec<[u64; 2]>>,
    weighted: Option<Vec<[u64; 2]>>,
}

/// A distribution of unsigned integers (GEN-2).
#[derive(Debug, Clone, PartialEq, Eq)]
pub enum Dist {
    Const(u64),
    Uniform {
        min: u64,
        max: u64,
    },
    /// `(p, v)` points, `p` in parts per million from 0 to [`PPM`].
    Quantiles(Vec<(u64, u64)>),
    /// `(value, weight)` entries and their total weight.
    Weighted {
        entries: Vec<(u64, u64)>,
        total: u64,
    },
}

impl Dist {
    fn parse(name: &str, raw: RawDist) -> Result<Self, GenError> {
        let bad = |m: &str| Err(GenError::Sheet(format!("`{name}`: {m} (GEN-2)")));
        let given = [
            raw.constant.is_some(),
            raw.uniform.is_some(),
            raw.quantiles.is_some(),
            raw.weighted.is_some(),
        ];
        if given.iter().filter(|g| **g).count() != 1 {
            return bad("a distribution is exactly one of const, uniform, quantiles, weighted");
        }
        if let Some(n) = raw.constant {
            return Ok(Self::Const(n));
        }
        if let Some([min, max]) = raw.uniform {
            if min > max || (min == 0 && max == u64::MAX) {
                return bad("uniform needs min <= max, and less than the whole 64-bit range");
            }
            return Ok(Self::Uniform { min, max });
        }
        if let Some(q) = raw.quantiles {
            let pts: Vec<(u64, u64)> = q.iter().map(|[p, v]| (*p, *v)).collect();
            if pts.len() < 2
                || pts.first().map(|x| x.0) != Some(0)
                || pts.last().map(|x| x.0) != Some(PPM)
            {
                return bad("quantiles run from p = 0 to p = 1000000, with at least two points");
            }
            if pts.windows(2).any(|w| w[0].0 >= w[1].0 || w[0].1 > w[1].1) {
                return bad("quantiles need strictly increasing p and non-decreasing values");
            }
            return Ok(Self::Quantiles(pts));
        }
        let entries: Vec<(u64, u64)> = raw
            .weighted
            .unwrap_or_default()
            .iter()
            .map(|[v, w]| (*v, *w))
            .collect();
        if entries.is_empty() || entries.iter().any(|e| e.1 == 0) {
            return bad("weighted needs at least one entry, every weight positive");
        }
        let mut seen = std::collections::BTreeSet::new();
        if entries.iter().any(|e| !seen.insert(e.0)) {
            return bad("weighted values must be distinct");
        }
        let Some(total) = entries.iter().try_fold(0u64, |t, e| t.checked_add(e.1)) else {
            return bad("weighted weights overflow 64 bits");
        };
        Ok(Self::Weighted { entries, total })
    }

    /// The smallest and largest values a draw can give.
    #[must_use]
    pub fn bounds(&self) -> (u64, u64) {
        match self {
            Self::Const(n) => (*n, *n),
            Self::Uniform { min, max } => (*min, *max),
            Self::Quantiles(p) => (p.first().map_or(0, |x| x.1), p.last().map_or(0, |x| x.1)),
            Self::Weighted { entries, .. } => (
                entries.iter().map(|e| e.0).min().unwrap_or(0),
                entries.iter().map(|e| e.0).max().unwrap_or(0),
            ),
        }
    }

    /// One draw from `rng` (GEN-2): integer arithmetic only.
    pub fn draw(&self, rng: &mut ChaCha20Rng) -> u64 {
        match self {
            Self::Const(n) => *n,
            Self::Uniform { min, max } => min + below(rng, max - min + 1),
            Self::Quantiles(pts) => {
                let u = below(rng, PPM + 1);
                let i = pts
                    .windows(2)
                    .position(|w| w[0].0 <= u && u <= w[1].0)
                    .unwrap_or(0);
                let ((p0, v0), (p1, v1)) = match (pts.get(i), pts.get(i + 1)) {
                    (Some(a), Some(b)) => (*a, *b),
                    _ => return pts.last().map_or(0, |x| x.1),
                };
                let num = u128::from(v1 - v0) * u128::from(u - p0);
                let step = num / u128::from(p1 - p0);
                v0 + u64::try_from(step).unwrap_or(v1 - v0)
            }
            Self::Weighted { entries, total } => {
                let r = below(rng, *total);
                let mut acc = 0u64;
                for (v, w) in entries {
                    acc += w;
                    if acc > r {
                        return *v;
                    }
                }
                entries.last().map_or(0, |e| e.0)
            }
        }
    }
}

/// The sheet as written (GEN-1).
#[derive(Debug, Deserialize)]
#[serde(deny_unknown_fields)]
struct RawSheet {
    schema_version: u32,
    placeholder: bool,
    doc: String,
    model: String,
    sessions: u64,
    system_tokens: u64,
    summary_instruction_tokens: u64,
    summary_max_tokens: u64,
    compact_at_tokens: u64,
    session_start_ns: RawDist,
    turns_per_session: RawDist,
    think_time_ns: RawDist,
    chain_length: RawDist,
    fanout_width: RawDist,
    user_tokens: RawDist,
    answer_tokens: RawDist,
    tool_class: RawClassChoice,
    tool_result_tokens: BTreeMap<String, RawDist>,
    tool_duration_ns: BTreeMap<String, RawDist>,
}

/// `tool_class` as written: `{ weighted = [["file", 3], …] }`.
#[derive(Debug, Deserialize)]
#[serde(deny_unknown_fields)]
struct RawClassChoice {
    weighted: Vec<(String, u64)>,
}

/// A loaded sheet (GEN-1).
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct Sheet {
    pub placeholder: bool,
    pub doc: String,
    pub model: String,
    pub sessions: u64,
    pub system_tokens: u64,
    pub summary_instruction_tokens: u64,
    pub summary_max_tokens: u64,
    pub compact_at_tokens: u64,
    pub session_start_ns: Dist,
    pub turns_per_session: Dist,
    pub think_time_ns: Dist,
    pub chain_length: Dist,
    pub fanout_width: Dist,
    pub user_tokens: Dist,
    pub answer_tokens: Dist,
    /// The drawable classes, in [`CLASSES`] order, and a choice over their
    /// indices in that list.
    pub classes: Vec<&'static str>,
    pub tool_class: Dist,
    pub tool_result_tokens: BTreeMap<&'static str, Dist>,
    pub tool_duration_ns: BTreeMap<&'static str, Dist>,
}

impl Sheet {
    /// Load and check the sheet at `path` (GEN-1, GEN-2) against the mock's
    /// embedded profiles (MLM-50).
    pub fn load(path: &Path) -> Result<Self, GenError> {
        let text = std::fs::read_to_string(path)
            .map_err(|e| GenError::Sheet(format!("{}: {e}", path.display())))?;
        let profiles =
            acn_mockllm::profile::embedded().map_err(|e| GenError::Internal(e.to_string()))?;
        Self::parse(&text, &profiles)
    }

    /// Parse and check sheet text against `profiles`.
    pub fn parse(text: &str, profiles: &acn_mockllm::profile::Profiles) -> Result<Self, GenError> {
        let raw: RawSheet = toml::from_str(text).map_err(|e| GenError::Sheet(e.to_string()))?;
        let refuse = |m: String| Err(GenError::Sheet(format!("{m} (GEN-1)")));
        if raw.schema_version != 1 {
            return refuse(format!("schema_version {} is not 1", raw.schema_version));
        }
        let Some(profile) = profiles.get(&raw.model) else {
            return refuse(format!(
                "`model` {} is not an embedded mock profile",
                raw.model
            ));
        };
        if raw.sessions == 0 {
            return refuse("`sessions` is at least 1".into());
        }
        let d = Dist::parse;
        let turns_per_session = d("turns_per_session", raw.turns_per_session)?;
        if turns_per_session.bounds().0 == 0 {
            return refuse("`turns_per_session` is at least 1".into());
        }
        let chain_length = d("chain_length", raw.chain_length)?;
        let fanout_width = d("fanout_width", raw.fanout_width)?;
        // The drawable classes, by name, weights carried over.
        let mut by_class: BTreeMap<usize, u64> = BTreeMap::new();
        for (name, w) in &raw.tool_class.weighted {
            let Some(i) = CLASSES.iter().position(|c| c == name) else {
                return refuse(format!("`tool_class`: `{name}` is not a TRC-13 class"));
            };
            if *name == SUBAGENT {
                return refuse("`tool_class` cannot draw `subagent`: fan-out spawns it".into());
            }
            if by_class.insert(i, *w).is_some() {
                return refuse(format!("`tool_class`: `{name}` is listed twice"));
            }
        }
        let classes: Vec<&'static str> = by_class
            .keys()
            .filter_map(|i| CLASSES.get(*i).copied())
            .collect();
        let tool_class = d(
            "tool_class",
            RawDist {
                constant: None,
                uniform: None,
                quantiles: None,
                weighted: Some(
                    raw.tool_class
                        .weighted
                        .iter()
                        .filter_map(|(n, w)| {
                            let i = classes.iter().position(|c| c == n)?;
                            Some([u64::try_from(i).ok()?, *w])
                        })
                        .collect(),
                ),
            },
        )?;
        let per_class = |what: &str, mut m: BTreeMap<String, RawDist>| {
            let mut out = BTreeMap::new();
            for c in &classes {
                let Some(raw) = m.remove(*c) else {
                    return Err(GenError::Sheet(format!(
                        "`{what}` has no entry for class `{c}`, which `tool_class` can draw (GEN-1)"
                    )));
                };
                out.insert(*c, Dist::parse(&format!("{what}.{c}"), raw)?);
            }
            if let Some(extra) = m.keys().next() {
                return Err(GenError::Sheet(format!(
                    "`{what}.{extra}` names a class `tool_class` cannot draw (GEN-1)"
                )));
            }
            Ok(out)
        };
        let tool_result_tokens = per_class("tool_result_tokens", raw.tool_result_tokens)?;
        let tool_duration_ns = per_class("tool_duration_ns", raw.tool_duration_ns)?;
        let session_start_ns = d("session_start_ns", raw.session_start_ns)?;
        let think_time_ns = d("think_time_ns", raw.think_time_ns)?;
        let user_tokens = d("user_tokens", raw.user_tokens)?;
        let answer_tokens = d("answer_tokens", raw.answer_tokens)?;
        // Sizes a plan or a text holds in memory are bounded (ADR-38).
        let mut sized: Vec<(String, u64, u64)> = vec![
            ("sessions".into(), raw.sessions, MAX_COUNT),
            (
                "turns_per_session".into(),
                turns_per_session.bounds().1,
                MAX_COUNT,
            ),
            ("fanout_width".into(), fanout_width.bounds().1, MAX_COUNT),
            ("system_tokens".into(), raw.system_tokens, MAX_TOKENS),
            (
                "summary_instruction_tokens".into(),
                raw.summary_instruction_tokens,
                MAX_TOKENS,
            ),
            (
                "summary_max_tokens".into(),
                raw.summary_max_tokens,
                MAX_TOKENS,
            ),
            ("user_tokens".into(), user_tokens.bounds().1, MAX_TOKENS),
            ("answer_tokens".into(), answer_tokens.bounds().1, MAX_TOKENS),
        ];
        for (c, dist) in &tool_result_tokens {
            sized.push((
                format!("tool_result_tokens.{c}"),
                dist.bounds().1,
                MAX_TOKENS,
            ));
        }
        if let Some((name, v, cap)) = sized.into_iter().find(|(_, v, cap)| v > cap) {
            return refuse(format!("`{name}` can be {v}, above its limit {cap}"));
        }
        // The spawn is a tool call (GEN-12).
        if fanout_width.bounds().1 > 0 && chain_length.bounds().0 == 0 {
            return refuse(
                "`fanout_width` can be above 0 while `chain_length` can be 0, but the spawn is a tool call".into(),
            );
        }
        // The mock replies with a tool call only while fewer results than
        // its limit follow the user message (MLM-40).
        let longest = chain_length.bounds().1;
        if longest > profile.tool_calls_per_turn {
            return refuse(format!(
                "`chain_length` can be {longest}, but profile {} allows {} tool calls per turn (MLM-40)",
                raw.model, profile.tool_calls_per_turn
            ));
        }
        Ok(Self {
            placeholder: raw.placeholder,
            doc: raw.doc,
            model: raw.model,
            sessions: raw.sessions,
            system_tokens: raw.system_tokens,
            summary_instruction_tokens: raw.summary_instruction_tokens,
            summary_max_tokens: raw.summary_max_tokens,
            compact_at_tokens: raw.compact_at_tokens,
            session_start_ns,
            turns_per_session,
            think_time_ns,
            chain_length,
            fanout_width,
            user_tokens,
            answer_tokens,
            classes,
            tool_class,
            tool_result_tokens,
            tool_duration_ns,
        })
    }

    /// The tools every call carries (GEN-11): the drawable classes, and
    /// `subagent` when a turn can fan out, in TRC-13's order.
    #[must_use]
    pub fn tool_classes(&self) -> Vec<&'static str> {
        CLASSES
            .iter()
            .copied()
            .filter(|c| {
                self.classes.contains(c) || (*c == SUBAGENT && self.fanout_width.bounds().1 > 0)
            })
            .collect()
    }
}
