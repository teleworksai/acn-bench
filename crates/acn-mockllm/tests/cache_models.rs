//! MLM-20..23: each cache model on hand-computed request sequences.
//!
//! A user message `{"content":"<C>","role":"user"}\n` is 29 + |C| bytes; with
//! |C| = 35 it is 64 bytes, 16 tokens. A second content that agrees on its first 20
//! characters diverges at byte 32, inside token 8.

#![allow(clippy::unwrap_used, clippy::expect_used, clippy::panic)] // CON-19: tests are exempt

mod common;

use common::{mock_with, profile_toml, user};
use serde_json::json;

const C: &str = "abcdefghijklmnopqrstuvwxyz012345678"; // 35 chars
const D: &str = "abcdefghijklmnopqrstZZZZZZZZZZZZZZZ"; // agrees on 20

fn cached(m: &mut acn_mockllm::Mock, body: &[u8], tenant: &str, at: i64) -> (u64, u64) {
    let o = m.handle(body, tenant, at);
    assert_eq!(o.status, 200, "{}", String::from_utf8_lossy(&o.body));
    (o.accounting.cached_tokens, o.accounting.cache_write_tokens)
}

/// Cites: MLM-20, MLM-22
#[test]
fn automatic_prefix_reads_the_longest_increment_and_expires() {
    let mut m = mock_with(&[profile_toml("a", "automatic_prefix", &[])], 1);
    assert_eq!(acn_mockllm::prompt::tokens_of(29 + C.len()), 16);
    assert_eq!(cached(&mut m, &user("a", C), "-", 0), (0, 0));
    assert_eq!(
        cached(&mut m, &user("a", C), "-", 1),
        (16, 0),
        "the whole prompt, a multiple of 4 above 8"
    );
    assert_eq!(
        cached(&mut m, &user("a", D), "-", 2),
        (8, 0),
        "diverges inside token 8: the 8-token prefix"
    );
    assert_eq!(
        cached(&mut m, &user("a", C), "other", 3),
        (0, 0),
        "tenants never share"
    );
    // ttl_ns = 1e6: the 16-token entry was last used at 1, the 8-token one at 2.
    assert_eq!(
        cached(&mut m, &user("a", C), "-", 1_000_002),
        (8, 0),
        "older than ttl expires"
    );
    assert_eq!(cached(&mut m, &user("a", C), "-", 3_000_000), (0, 0));
    // Below the minimum nothing is cached: 16 tokens under a 32-token minimum.
    let mut m = mock_with(
        &[profile_toml(
            "a",
            "automatic_prefix",
            &["min_cacheable_tokens = 32"],
        )],
        1,
    );
    assert_eq!(cached(&mut m, &user("a", C), "-", 0), (0, 0));
    assert_eq!(cached(&mut m, &user("a", C), "-", 1), (0, 0));
}

fn with_system(system: &str, users: &[(&str, bool)], model: &str) -> Vec<u8> {
    let mut msgs = vec![
        json!({ "role": "system", "content": system, "cache_control": { "type": "ephemeral" } }),
    ];
    for (u, mark) in users {
        let mut m = json!({ "role": "user", "content": u });
        if *mark {
            m["cache_control"] = json!({ "type": "ephemeral" });
        }
        msgs.push(m);
    }
    json!({ "model": model, "messages": msgs })
        .to_string()
        .into_bytes()
}

/// Cites: MLM-20, MLM-21
#[test]
fn explicit_breakpoints_cache_only_what_is_marked() {
    let mut m = mock_with(&[profile_toml("e", "explicit_breakpoints", &[])], 1);
    // The system element is 31 + |S| bytes: 64 with 33 characters, 16 tokens.
    let s33 = "s".repeat(33);
    assert_eq!(
        cached(&mut m, &with_system(&s33, &[("a", false)], "e"), "-", 0),
        (0, 16)
    );
    assert_eq!(
        cached(&mut m, &with_system(&s33, &[("b", false)], "e"), "-", 1),
        (16, 0),
        "the user turn changed, the marked prefix did not"
    );
    // A breakpoint shorter than the minimum (31 bytes, 7 tokens) is ignored.
    assert_eq!(
        cached(&mut m, &with_system("", &[("a", false)], "e"), "-", 2),
        (0, 0)
    );
    assert_eq!(
        cached(&mut m, &with_system("", &[("a", false)], "e"), "-", 3),
        (0, 0)
    );
    // Without a breakpoint nothing is cached.
    assert_eq!(cached(&mut m, &user("e", C), "-", 4), (0, 0));
    assert_eq!(cached(&mut m, &user("e", C), "-", 5), (0, 0));
    // More breakpoints than allowed is a 400.
    let o = m.handle(&with_system(&s33, &[("a", true), ("b", true)], "e"), "-", 6);
    assert_eq!(o.status, 400);
    assert!(String::from_utf8_lossy(&o.body).contains("at most 2"));
}

/// Cites: MLM-20, MLM-23
#[test]
fn block_granular_reads_leading_blocks_and_evicts_leaves_first() {
    // block 4 tokens, capacity 5 blocks; a 16-token prompt is 4 blocks.
    let mut m = mock_with(&[profile_toml("b", "block_granular", &[])], 1);
    assert_eq!(cached(&mut m, &user("b", C), "-", 0), (0, 16));
    assert_eq!(cached(&mut m, &user("b", C), "-", 1), (16, 0));
    // D shares blocks 1-2: 2 new blocks make 6 > 5, so the oldest leaf (C's block 4) goes.
    assert_eq!(cached(&mut m, &user("b", D), "-", 2), (8, 8));
    assert_eq!(m.cache_sizes().1, 5);
    // C: blocks 1-3 remain (block 3 had a child, so it was not a leaf); block 4 again.
    assert_eq!(cached(&mut m, &user("b", C), "-", 3), (12, 4));
    // Now D's block 4 was the oldest leaf and went; D reads its three leading blocks.
    assert_eq!(cached(&mut m, &user("b", D), "-", 4), (12, 4));
    assert_eq!(
        cached(&mut m, &user("b", C), "other", 5),
        (0, 16),
        "tenants never share blocks"
    );
}

/// Cites: MLM-20
#[test]
fn cached_tokens_never_exceed_the_prompt() {
    for model in ["automatic_prefix", "block_granular"] {
        let mut m = mock_with(&[profile_toml("p", model, &[])], 1);
        for at in 0..3 {
            let o = m.handle(&user("p", C), "-", at);
            let usage = &common::body(&o)["usage"];
            assert!(
                usage["prompt_tokens_details"]["cached_tokens"].as_u64()
                    <= usage["prompt_tokens"].as_u64()
            );
        }
    }
}
