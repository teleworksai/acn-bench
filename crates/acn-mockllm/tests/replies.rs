//! MLM-40, MLM-41: the reply policy and the fault model.

#![allow(clippy::unwrap_used, clippy::expect_used, clippy::panic)] // CON-19: tests are exempt

mod common;

use common::{body, header, mock_with, profile_toml, user};
use serde_json::json;

fn with_tools(results_since_user: usize) -> Vec<u8> {
    let mut messages = vec![json!({ "role": "user", "content": "go" })];
    for i in 0..results_since_user {
        messages.push(json!({ "role": "assistant", "content": null, "tool_calls": [{ "id": format!("c{i}"), "type": "function", "function": { "name": "x", "arguments": "{}" } }] }));
        messages.push(json!({ "role": "tool", "tool_call_id": format!("c{i}"), "content": "ok" }));
    }
    json!({
        "model": "r", "messages": messages,
        "tools": [
            { "type": "function", "function": { "name": "read", "parameters": {} } },
            { "type": "function", "function": { "name": "grep", "parameters": {} } }
        ]
    })
    .to_string()
    .into_bytes()
}

/// Cites: MLM-40
#[test]
fn tools_are_called_in_turn_then_the_turn_is_answered() {
    let mut m = mock_with(
        &[profile_toml(
            "r",
            "automatic_prefix",
            &["tool_calls_per_turn = 2"],
        )],
        3,
    );
    let first = body(&m.handle(&with_tools(0), "-", 0));
    assert_eq!(first["choices"][0]["finish_reason"], "tool_calls");
    assert_eq!(
        first["choices"][0]["message"]["tool_calls"][0]["function"]["name"],
        "read"
    );
    assert_eq!(
        first["choices"][0]["message"]["tool_calls"][0]["function"]["arguments"],
        "{}"
    );
    assert_eq!(
        first["usage"]["completion_tokens"], 1,
        "a tool call is one token"
    );
    let second = body(&m.handle(&with_tools(1), "-", 1));
    assert_eq!(
        second["choices"][0]["message"]["tool_calls"][0]["function"]["name"],
        "grep"
    );
    let third = body(&m.handle(&with_tools(2), "-", 2));
    assert_eq!(third["choices"][0]["finish_reason"], "stop");
    let text = third["choices"][0]["message"]["content"].as_str().unwrap();
    assert_eq!(text.len(), 4 * 5, "five 4-byte words");
    assert_eq!(third["usage"]["completion_tokens"], 5);
}

/// Cites: MLM-40
#[test]
fn a_token_limit_cuts_the_answer_with_length() {
    let mut m = mock_with(&[profile_toml("r", "automatic_prefix", &[])], 3);
    let req =
        json!({ "model": "r", "max_tokens": 3, "messages": [{ "role": "user", "content": "go" }] });
    let b = body(&m.handle(req.to_string().as_bytes(), "-", 0));
    assert_eq!(b["choices"][0]["finish_reason"], "length");
    assert_eq!(b["usage"]["completion_tokens"], 3);
    assert_eq!(
        b["choices"][0]["message"]["content"]
            .as_str()
            .unwrap()
            .len(),
        12
    );
}

/// Cites: MLM-41
#[test]
fn faults_are_injected_at_their_rates_and_never_touch_the_cache() {
    let mut m = mock_with(
        &[profile_toml(
            "f",
            "automatic_prefix",
            &["fault_429_ppm = 1000000", "retry_after_s_max = 3"],
        )],
        5,
    );
    let o = m.handle(&user("f", "abcdefghijklmnopqrstuvwxyz012345678"), "-", 0);
    assert_eq!(o.status, 429);
    let retry: u64 = header(&o, "retry-after").unwrap().parse().unwrap();
    assert!((1..=3).contains(&retry));
    assert_eq!(body(&o)["error"]["type"], "rate_limit_error");
    assert_eq!(m.cache_sizes(), (0, 0));

    let mut m = mock_with(
        &[profile_toml(
            "f",
            "automatic_prefix",
            &["fault_500_ppm = 1000000"],
        )],
        5,
    );
    assert_eq!(m.handle(&user("f", "x"), "-", 0).status, 500);

    let mut m = mock_with(
        &[profile_toml(
            "f",
            "automatic_prefix",
            &["fault_cut_ppm = 1000000"],
        )],
        5,
    );
    let req = json!({ "model": "f", "stream": true, "messages": [{ "role": "user", "content": "abcdefghijklmnopqrstuvwxyz012345678" }] });
    let o = m.handle(req.to_string().as_bytes(), "-", 0);
    assert_eq!(o.status, 200);
    assert!(
        o.chunks.iter().all(|c| c.data != "[DONE]"),
        "a cut stream ends without [DONE]"
    );
    assert!(
        o.chunks
            .iter()
            .all(|c| !c.data.contains("\"finish_reason\":\"stop\""))
    );
    assert!(!o.chunks.is_empty() && o.chunks.len() <= 5);
    assert_eq!(m.cache_sizes(), (0, 0), "a cut changes no cache state");
    // Not streamed, a cut does not apply.
    assert_eq!(
        body(&m.handle(&user("f", "x"), "-", 1))["choices"][0]["finish_reason"],
        "stop"
    );
}

/// Cites: MLM-40
#[test]
fn the_tool_index_wraps_and_the_call_id_comes_from_the_prompt_bytes() {
    let mut m = mock_with(
        &[profile_toml(
            "r",
            "automatic_prefix",
            &["tool_calls_per_turn = 3"],
        )],
        3,
    );
    let req = with_tools(2);
    let third = body(&m.handle(&req, "-", 0));
    let call = &third["choices"][0]["message"]["tool_calls"][0];
    assert_eq!(call["function"]["name"], "read", "2 results mod 2 tools");
    let v: serde_json::Value = serde_json::from_slice(&req).unwrap();
    let p = acn_mockllm::prompt::prompt(&v, m.profiles().get("r").unwrap()).unwrap();
    assert_eq!(
        call["id"],
        format!("call_{}", &blake3::hash(&p.bytes).to_hex()[..16])
    );
}

/// Cites: MLM-40
#[test]
fn only_tool_results_since_the_last_user_message_count() {
    let mut m = mock_with(&[profile_toml("r", "automatic_prefix", &[])], 3);
    // A tool result, then a new user turn: no result since it, so a tool call.
    let req = json!({ "model": "r", "messages": [
        { "role": "user", "content": "go" },
        { "role": "assistant", "content": null, "tool_calls": [{ "id": "c0", "type": "function", "function": { "name": "read", "arguments": "{}" } }] },
        { "role": "tool", "tool_call_id": "c0", "content": "ok" },
        { "role": "user", "content": "again" }
    ], "tools": [{ "type": "function", "function": { "name": "read", "parameters": {} } }] });
    let b = body(&m.handle(req.to_string().as_bytes(), "-", 0));
    assert_eq!(b["choices"][0]["finish_reason"], "tool_calls");
}

/// Cites: MLM-41, MLM-6
#[test]
fn a_fault_rate_fires_near_its_rate_and_shifts_no_other_draw() {
    let mut m = mock_with(
        &[profile_toml(
            "f",
            "automatic_prefix",
            &["fault_429_ppm = 250000"],
        )],
        11,
    );
    let n = (0..2000)
        .filter(|i| m.handle(&user("f", "x"), "-", *i).status == 429)
        .count();
    assert!((400..=600).contains(&n), "{n} of 2000 at 25%");
    // A rate that never fires here leaves every other draw where it was.
    let run = |extra: &[&str]| {
        let mut m = mock_with(&[profile_toml("f", "automatic_prefix", extra)], 11);
        (0..50)
            .map(|i| m.handle(&user("f", "x"), "-", i).body)
            .collect::<Vec<_>>()
    };
    assert_eq!(run(&[]), run(&["fault_500_ppm = 1"]));
}

/// Cites: MLM-41
#[test]
fn a_cut_stream_has_no_usage_and_a_500_leaves_the_cache_alone() {
    let mut m = mock_with(
        &[profile_toml(
            "f",
            "automatic_prefix",
            &["fault_cut_ppm = 1000000"],
        )],
        5,
    );
    let req = json!({ "model": "f", "stream": true, "stream_options": { "include_usage": true },
        "messages": [{ "role": "user", "content": "abcdefghijklmnopqrstuvwxyz012345678" }] });
    let o = m.handle(req.to_string().as_bytes(), "-", 0);
    assert!(o.chunks.iter().all(|c| !c.data.contains("\"usage\"")));

    let mut m = mock_with(
        &[profile_toml(
            "f",
            "automatic_prefix",
            &["fault_500_ppm = 1000000"],
        )],
        5,
    );
    let o = m.handle(&user("f", "abcdefghijklmnopqrstuvwxyz012345678"), "-", 0);
    assert_eq!(o.status, 500);
    assert_eq!(body(&o)["error"]["type"], "server_error");
    assert_eq!(m.cache_sizes(), (0, 0));
}
