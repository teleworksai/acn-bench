//! Profiles (MLM-50): data embedded in the crate, every parameter explicit.

use serde::Deserialize;

/// The embedded profiles file.
pub const PROFILES_TOML: &str = include_str!("profiles.toml");

/// A profile that cannot be used.
#[derive(Debug, thiserror::Error)]
pub enum ProfileError {
    #[error("profiles.toml: {0}")]
    Invalid(String),
}

/// The cache model of a profile (MLM-20).
#[derive(Debug, Clone, Copy, PartialEq, Eq, Deserialize)]
#[serde(rename_all = "snake_case")]
pub enum CacheModel {
    ExplicitBreakpoints,
    AutomaticPrefix,
    BlockGranular,
}

/// A prompt segment, for `prefix_order` (MLM-10).
#[derive(Debug, Clone, Copy, PartialEq, Eq, Deserialize)]
#[serde(rename_all = "snake_case")]
pub enum Segment {
    Tools,
    System,
    Messages,
}

/// One profile. Every field is required: nothing is implied at load time.
#[derive(Debug, Clone, Deserialize)]
#[serde(deny_unknown_fields)]
pub struct Profile {
    pub name: String,
    /// The constants are not yet calibrated (MLM-50).
    pub placeholder: bool,
    pub doc: String,
    pub cache_model: CacheModel,
    pub prefix_order: Vec<Segment>,
    pub ttl_ns: i64,
    pub min_cacheable_tokens: u64,
    pub increment_tokens: u64,
    pub max_breakpoints: u64,
    pub block_tokens: u64,
    pub capacity_blocks: u64,
    /// Requests in service at once; 0 is unlimited (MLM-30).
    pub slots: u64,
    pub prefill_base_ns: i64,
    pub prefill_ns_per_new_token: i64,
    pub prefill_ns_per_cached_token: i64,
    pub itl_ns: i64,
    pub itl_jitter_ns: i64,
    pub tool_calls_per_turn: u64,
    pub output_tokens_min: u64,
    pub output_tokens_max: u64,
    pub fault_429_ppm: u64,
    pub fault_500_ppm: u64,
    pub fault_cut_ppm: u64,
    pub retry_after_s_max: u64,
}

#[derive(Debug, Deserialize)]
#[serde(deny_unknown_fields)]
struct ProfilesFile {
    schema_version: u32,
    #[serde(rename = "profile")]
    profiles: Vec<Profile>,
}

/// Fault rates are in parts per million (MLM-41).
pub const PPM: u64 = 1_000_000;

/// The loaded profiles and the BLAKE3 of the file they came from.
#[derive(Debug, Clone)]
pub struct Profiles {
    pub profiles: Vec<Profile>,
    pub blake3: String,
}

impl Profiles {
    /// Parse and check a profiles file.
    pub fn parse(text: &str) -> Result<Self, ProfileError> {
        let file: ProfilesFile =
            toml::from_str(text).map_err(|e| ProfileError::Invalid(e.to_string()))?;
        if file.schema_version != 1 {
            return Err(ProfileError::Invalid(format!(
                "schema_version {} is not supported",
                file.schema_version
            )));
        }
        let profiles = Self {
            profiles: file.profiles,
            blake3: blake3::hash(text.as_bytes()).to_hex().to_string(),
        };
        profiles.check()?;
        Ok(profiles)
    }

    /// Check the invariants the engine relies on. [`Profiles::parse`] runs it, and
    /// so does [`crate::Mock::with_profiles`], since the fields are public.
    pub fn check(&self) -> Result<(), ProfileError> {
        let bad = |m: String| Err(ProfileError::Invalid(m));
        let mut names = std::collections::BTreeSet::new();
        for p in &self.profiles {
            let at = &p.name;
            if !names.insert(p.name.as_str()) {
                return bad(format!("profile `{at}` is defined twice"));
            }
            let mut order = p.prefix_order.clone();
            order.sort_by_key(|s| *s as u8);
            order.dedup();
            if order.len() != 3 || p.prefix_order.len() != 3 {
                return bad(format!(
                    "profile `{at}`: prefix_order names each of tools, system, messages once"
                ));
            }
            let positive = [
                ("ttl_ns", p.ttl_ns > 0),
                ("increment_tokens", p.increment_tokens > 0),
                ("max_breakpoints", p.max_breakpoints > 0),
                ("block_tokens", p.block_tokens > 0),
                ("capacity_blocks", p.capacity_blocks > 0),
                ("itl_ns", p.itl_ns > 0),
                ("output_tokens_min", p.output_tokens_min > 0),
                ("retry_after_s_max", p.retry_after_s_max > 0),
            ];
            if let Some((name, _)) = positive.iter().find(|(_, ok)| !ok) {
                return bad(format!("profile `{at}`: `{name}` must be positive"));
            }
            let non_negative = [
                p.prefill_base_ns,
                p.prefill_ns_per_new_token,
                p.prefill_ns_per_cached_token,
                p.itl_jitter_ns,
            ];
            if non_negative.iter().any(|v| *v < 0) {
                return bad(format!("profile `{at}`: a timing constant is negative"));
            }
            if p.itl_jitter_ns >= p.itl_ns {
                return bad(format!(
                    "profile `{at}`: itl_jitter_ns must be below itl_ns"
                ));
            }
            if p.output_tokens_min > p.output_tokens_max {
                return bad(format!(
                    "profile `{at}`: output_tokens_min exceeds output_tokens_max"
                ));
            }
            if [p.fault_429_ppm, p.fault_500_ppm, p.fault_cut_ppm]
                .iter()
                .sum::<u64>()
                > PPM
            {
                return bad(format!(
                    "profile `{at}`: the fault rates add up to more than 1e6 ppm"
                ));
            }
        }
        Ok(())
    }

    /// The profile named `name`.
    #[must_use]
    pub fn get(&self, name: &str) -> Option<&Profile> {
        self.profiles.iter().find(|p| p.name == name)
    }
}

/// The profiles embedded in this build.
pub fn embedded() -> Result<Profiles, ProfileError> {
    Profiles::parse(PROFILES_TOML)
}
