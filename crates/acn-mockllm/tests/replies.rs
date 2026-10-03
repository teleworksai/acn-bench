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
