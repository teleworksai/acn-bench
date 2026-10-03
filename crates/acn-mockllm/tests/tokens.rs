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
    assert_eq!(
        b.elements,
        vec![(a.bytes.len(), true)],
        "the element carries the breakpoint"
    );
    assert!(
        prompt::prompt(&json!({ "model": "p" }), &pr).is_err(),
        "no messages"
    );
    assert!(prompt::prompt(&json!({ "model": "p", "messages": [], "tools": {} }), &pr).is_err());
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
