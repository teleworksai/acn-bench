//! Helpers shared by the mock's tests.
#![allow(dead_code, clippy::unwrap_used, clippy::expect_used, clippy::panic)]

use acn_mockllm::Mock;
use acn_mockllm::profile::Profiles;
use serde_json::{Value, json};

/// A profile with small constants for hand-computed tests; `overrides` are
/// `key = value` TOML lines that replace the defaults below.
pub fn profile_toml(name: &str, cache_model: &str, overrides: &[&str]) -> String {
    let mut fields = vec![
        ("placeholder", "true".to_owned()),
        ("doc", "\"test\"".to_owned()),
        ("cache_model", format!("\"{cache_model}\"")),
        (
            "prefix_order",
            "[\"tools\", \"system\", \"messages\"]".to_owned(),
        ),
        ("ttl_ns", "1000000".to_owned()),
        ("min_cacheable_tokens", "8".to_owned()),
        ("increment_tokens", "4".to_owned()),
        ("max_breakpoints", "2".to_owned()),
        ("block_tokens", "4".to_owned()),
        ("capacity_blocks", "5".to_owned()),
        ("slots", "0".to_owned()),
        ("prefill_base_ns", "1000".to_owned()),
        ("prefill_ns_per_new_token", "10".to_owned()),
        ("prefill_ns_per_cached_token", "1".to_owned()),
        ("itl_ns", "100".to_owned()),
        ("itl_jitter_ns", "0".to_owned()),
        ("tool_calls_per_turn", "1".to_owned()),
        ("output_tokens_min", "5".to_owned()),
        ("output_tokens_max", "5".to_owned()),
        ("fault_429_ppm", "0".to_owned()),
        ("fault_500_ppm", "0".to_owned()),
        ("fault_cut_ppm", "0".to_owned()),
        ("retry_after_s_max", "5".to_owned()),
    ];
    for o in overrides {
        let (k, v) = o.split_once('=').unwrap();
        let k = k.trim();
        let slot = fields.iter_mut().find(|(f, _)| *f == k).unwrap();
        slot.1 = v.trim().to_owned();
    }
    let mut s = format!("[[profile]]\nname = \"{name}\"\n");
    for (k, v) in fields {
        s.push_str(&format!("{k} = {v}\n"));
    }
    s
}

pub fn mock_with(profiles: &[String], seed: u64) -> Mock {
    let text = format!("schema_version = 1\n\n{}", profiles.join("\n"));
    Mock::with_profiles(Profiles::parse(&text).unwrap(), seed).unwrap()
}

/// A request with one user message whose content is `content`.
pub fn user(model: &str, content: &str) -> Vec<u8> {
    json!({ "model": model, "messages": [{ "role": "user", "content": content }] })
        .to_string()
        .into_bytes()
}

pub fn body(o: &acn_mockllm::Outcome) -> Value {
    serde_json::from_slice(&o.body).unwrap()
}

pub fn header<'a>(o: &'a acn_mockllm::Outcome, name: &str) -> Option<&'a str> {
    o.headers
        .iter()
        .find(|(k, _)| k == name)
        .map(|(_, v)| v.as_str())
}
