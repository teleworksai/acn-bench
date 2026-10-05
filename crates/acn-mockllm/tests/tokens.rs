//! MLM-10, MLM-11: canonical prompt bytes and the 4-byte tokenizer.

#![allow(clippy::unwrap_used, clippy::expect_used, clippy::panic)] // CON-19: tests are exempt

mod common;

use acn_mockllm::profile::Profiles;
use acn_mockllm::prompt;
use serde_json::json;

fn profile(order: &str) -> acn_mockllm::profile::Profile {
    let t = common::profile_toml(
        "p",
        "automatic_prefix",
        &[&format!("prefix_order = {order}")],
    );
    Profiles::parse(&format!("schema_version = 1\n{t}"))
        .unwrap()
        .profiles[0]
        .clone()
}

/// Cites: MLM-10
#[test]
fn prompt_bytes_are_canonical_json_in_prefix_order() {
    let req = json!({
        "model": "p",
        "messages": [
            { "role": "user", "content": "hi" },
            { "role": "system", "content": "S" }
        ],
        "tools": [{ "type": "function", "function": { "name": "f", "parameters": { "minimum": 0.5, "type": "object" } } }]
    });
    let p = prompt::prompt(&req, &profile("[\"tools\", \"system\", \"messages\"]")).unwrap();
    let expected = concat!(
        "{\"function\":{\"name\":\"f\",\"parameters\":{\"minimum\":0.5,\"type\":\"object\"}},\"type\":\"function\"}\n",
        "{\"content\":\"S\",\"role\":\"system\"}\n",
        "{\"content\":\"hi\",\"role\":\"user\"}\n",
    );
    assert_eq!(
        String::from_utf8(p.bytes.clone()).unwrap(),
        expected,
        "sorted keys, system lifted, numbers as CON-27(c)"
    );
    // Another order is another prefix.
    let q = prompt::prompt(&req, &profile("[\"system\", \"tools\", \"messages\"]")).unwrap();
    assert!(
        String::from_utf8(q.bytes)
            .unwrap()
            .starts_with("{\"content\":\"S\"")
    );
}

/// Cites: MLM-10
#[test]
fn a_breakpoint_never_changes_the_bytes_and_bad_shapes_are_refused() {
    let plain = json!({ "model": "p", "messages": [{ "role": "system", "content": [{ "type": "text", "text": "S" }] }] });
    let marked = json!({ "model": "p", "messages": [{ "role": "system", "content": [{ "type": "text", "text": "S", "cache_control": { "type": "ephemeral" } }] }] });
    let pr = profile("[\"tools\", \"system\", \"messages\"]");
    let a = prompt::prompt(&plain, &pr).unwrap();
    let b = prompt::prompt(&marked, &pr).unwrap();
    assert_eq!(a.bytes, b.bytes);
    let text = String::from_utf8(b.bytes.clone()).unwrap();
    assert_eq!(
        text,
        "{\"content\":[{\"text\":\"S\",\"type\":\"text\"}],\"role\":\"system\"}\n"
    );
    assert_eq!(
        b.breakpoints,
        vec![text.find("}]").unwrap() + 1],
        "a marked part's prefix ends with the part"
    );
    // A marked message or tool ends with its element, `\n` included.
    let msg = json!({ "model": "p", "messages": [{ "role": "user", "content": "x", "cache_control": { "type": "ephemeral" } }] });
    let m = prompt::prompt(&msg, &pr).unwrap();
    assert_eq!(m.breakpoints, vec![m.bytes.len()]);
    let tool = json!({ "model": "p", "messages": [{ "role": "user", "content": "x" }],
        "tools": [{ "type": "function", "function": { "name": "f" }, "cache_control": { "type": "ephemeral" } }] });
    let t = prompt::prompt(&tool, &pr).unwrap();
    assert_eq!(
        t.breakpoints,
        vec![
            String::from_utf8(t.bytes.clone())
                .unwrap()
                .find('\n')
                .unwrap()
                + 1
        ]
    );
    assert!(a.breakpoints.is_empty());
}

/// Cites: MLM-1
#[test]
fn a_prompt_mlm_1_does_not_admit_is_refused() {
    let pr = profile("[\"tools\", \"system\", \"messages\"]");
    for (bad, why) in [
        (json!({ "model": "p" }), "no messages"),
        (
            json!({ "model": "p", "messages": [], "tools": {} }),
            "tools not an array",
        ),
        (
            json!({ "model": "p", "messages": [1] }),
            "a message that is not an object",
        ),
        (
            json!({ "model": "p", "messages": [{ "role": "robot", "content": "x" }] }),
            "an unknown role",
        ),
        (
            json!({ "model": "p", "messages": [{ "content": "x" }] }),
            "no role",
        ),
        (
            json!({ "model": "p", "messages": [{ "role": "user", "content": 7 }] }),
            "numeric content",
        ),
        (
            json!({ "model": "p", "messages": [{ "role": "user", "content": [1] }] }),
            "a part that is not an object",
        ),
        (
            json!({ "model": "p", "messages": [], "tools": [{ "type": "function", "function": {} }] }),
            "a tool without a name",
        ),
    ] {
        assert!(prompt::prompt(&bad, &pr).is_err(), "{why}");
    }
    let ok = json!({ "model": "p", "messages": [
        { "role": "user", "content": [{ "type": "text", "text": "x" }] },
        { "role": "assistant", "content": null, "tool_calls": [] },
        { "role": "tool", "tool_call_id": "c", "content": "ok" }
    ] });
    assert!(prompt::prompt(&ok, &pr).is_ok());
}

/// Cites: MLM-10
#[test]
fn numbers_have_one_spelling() {
    // 1e21 and 5.0 have one spelling each.
    let mut s = String::new();
    prompt::canonical(&json!({ "a": 1e21, "b": 5.0, "c": -0.0 }), &mut s).unwrap();
    assert_eq!(s, "{\"a\":1e21,\"b\":5.0,\"c\":0.0}");
}

/// Cites: MLM-11
#[test]
fn a_token_is_four_bytes_and_the_last_may_be_shorter() {
    assert_eq!(prompt::tokens_of(0), 0);
    assert_eq!(prompt::tokens_of(4), 1);
    assert_eq!(prompt::tokens_of(10), 3);
    let t = prompt::tokenize(b"0123456789");
    assert_eq!(t, vec![&b"0123"[..], &b"4567"[..], &b"89"[..]]);
}

/// Cites: MLM-10, MLM-40
#[test]
fn a_tool_choice_never_changes_the_bytes() {
    let base = serde_json::json!({
        "model": "m",
        "messages": [{ "role": "user", "content": "hi" }],
        "tools": [{ "type": "function", "function": { "name": "t", "parameters": {} } }],
    });
    let bytes = |v: &serde_json::Value| {
        acn_mockllm::prompt::prompt(v, &profile("[\"tools\", \"system\", \"messages\"]"))
            .unwrap()
            .bytes
    };
    for c in [
        serde_json::json!("none"),
        serde_json::json!({ "type": "none" }),
        serde_json::json!({ "type": "allowed_tools", "allowed_tools": { "mode": "auto", "tools": [] } }),
    ] {
        let mut v = base.clone();
        v["tool_choice"] = c;
        assert_eq!(bytes(&v), bytes(&base));
    }
}
