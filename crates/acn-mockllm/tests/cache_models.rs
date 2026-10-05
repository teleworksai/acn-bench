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

fn ephemeral() -> serde_json::Value {
    json!({ "type": "ephemeral" })
}

/// A system message of two text parts, `p` then `q`, the first marked when `mark`.
fn two_parts(model: &str, p: &str, q: &str, mark: bool) -> Vec<u8> {
    let mut first = json!({ "type": "text", "text": p });
    if mark {
        first["cache_control"] = ephemeral();
    }
    json!({ "model": model, "messages": [
        { "role": "system", "content": [first, { "type": "text", "text": q }] },
        { "role": "user", "content": "go" }
    ] })
    .to_string()
    .into_bytes()
}

/// Cites: MLM-21
#[test]
fn a_breakpoint_on_a_content_part_marks_the_prefix_ending_with_that_part() {
    let mut m = mock_with(&[profile_toml("e", "explicit_breakpoints", &[])], 1);
    // `{"content":[{"text":"` is 21 bytes and `","type":"text"}` 16: with 35
    // characters the first part ends at byte 72, 18 tokens.
    assert_eq!(
        cached(&mut m, &two_parts("e", C, "a", true), "-", 0),
        (0, 18)
    );
    assert_eq!(
        cached(&mut m, &two_parts("e", C, "b", true), "-", 1),
        (18, 0),
        "the second part changed, the marked first part did not"
    );
}

/// Cites: MLM-21
#[test]
fn a_breakpoint_on_a_tool_definition_marks_the_prefix_ending_with_it() {
    let mut m = mock_with(&[profile_toml("e", "explicit_breakpoints", &[])], 1);
    // `{"function":{"name":"` 21 + 40 + `"},"type":"function"}` 21 + `\n`: 83 bytes, 20 tokens.
    let req = |u: &str| {
        json!({ "model": "e", "messages": [{ "role": "user", "content": u }],
            "tools": [{ "type": "function", "function": { "name": "f".repeat(40) }, "cache_control": ephemeral() }] })
        .to_string()
        .into_bytes()
    };
    assert_eq!(cached(&mut m, &req("a"), "-", 0), (0, 20));
    assert_eq!(cached(&mut m, &req("b"), "-", 1), (20, 0));
}

/// Cites: MLM-21
#[test]
fn every_cache_control_member_is_a_breakpoint_and_too_many_is_a_400_before_any_fault() {
    let cc = ephemeral();
    let req = json!({ "model": "e", "messages": [{ "role": "user", "content": [
        { "type": "text", "text": "a".repeat(40), "cache_control": cc },
        { "type": "text", "text": "b", "cache_control": cc }
    ] }] })
    .to_string()
    .into_bytes();
    let mut m = mock_with(
        &[profile_toml(
            "e",
            "explicit_breakpoints",
            &["max_breakpoints = 1"],
        )],
        1,
    );
    let o = m.handle(&req, "-", 0);
    assert_eq!(o.status, 400, "two marked parts in one message are two");
    assert!(String::from_utf8_lossy(&o.body).contains("2 cache_control breakpoints"));
    // An injected 429 never hides the 400.
    let mut m = mock_with(
        &[profile_toml(
            "e",
            "explicit_breakpoints",
            &["max_breakpoints = 1", "fault_429_ppm = 1000000"],
        )],
        1,
    );
    assert_eq!(m.handle(&req, "-", 0).status, 400);
    // The other models ignore breakpoints: no limit applies.
    let auto = String::from_utf8(req)
        .unwrap()
        .replace("\"model\":\"e\"", "\"model\":\"a\"");
    let mut m = mock_with(
        &[profile_toml(
            "a",
            "automatic_prefix",
            &["max_breakpoints = 1"],
        )],
        1,
    );
    assert_eq!(m.handle(auto.as_bytes(), "-", 0).status, 200);
}

/// Cites: MLM-21
#[test]
fn a_request_can_read_a_shorter_marked_prefix_and_write_a_longer_one() {
    let mut m = mock_with(&[profile_toml("e", "explicit_breakpoints", &[])], 1);
    let s33 = "s".repeat(33);
    assert_eq!(
        cached(&mut m, &with_system(&s33, &[("a", false)], "e"), "-", 0),
        (0, 16)
    );
    // The system element is 16 tokens; the user element `{"content":C,"role":"user"}\n`
    // another 16. cache_write_tokens is what is written beyond the read: the
    // longest prefix written, 32, less the 16 read (MLM-21 v0.3, issue #12), so
    // that uncached, read and written partition the prompt.
    assert_eq!(
        cached(&mut m, &with_system(&s33, &[(C, true)], "e"), "-", 1),
        (16, 16)
    );
    assert_eq!(
        cached(&mut m, &with_system(&s33, &[(C, true)], "e"), "-", 2),
        (32, 0)
    );
}

/// Cites: MLM-22
#[test]
fn automatic_prefix_rounds_down_to_the_increment_grid() {
    let mut m = mock_with(&[profile_toml("a", "automatic_prefix", &[])], 1);
    let c39 = format!("{C}9999"); // 29 + 39 = 68 bytes, 17 tokens
    assert_eq!(acn_mockllm::prompt::tokens_of(29 + c39.len()), 17);
    assert_eq!(cached(&mut m, &user("a", &c39), "-", 0), (0, 0));
    assert_eq!(
        cached(&mut m, &user("a", &c39), "-", 1),
        (16, 0),
        "8 + 2 x 4, not 17"
    );
}

/// Cites: MLM-23
#[test]
fn blocks_expire_by_ttl() {
    let mut m = mock_with(&[profile_toml("b", "block_granular", &[])], 1);
    assert_eq!(cached(&mut m, &user("b", C), "-", 0), (0, 16));
    assert_eq!(cached(&mut m, &user("b", C), "-", 1_000_000), (16, 0));
    assert_eq!(
        cached(&mut m, &user("b", C), "-", 2_000_001),
        (0, 16),
        "last used at 1e6, older than the 1e6 ttl"
    );
}

/// The block hash of SPEC 030 as ADR-16 states it: tenant length, tenant, prefix.
fn block_hash(tenant: &str, prefix: &[u8]) -> [u8; 32] {
    let mut h = blake3::Hasher::new();
    h.update(&(tenant.len() as u64).to_le_bytes());
    h.update(tenant.as_bytes());
    h.update(prefix);
    *h.finalize().as_bytes()
}

/// Cites: MLM-23
#[test]
fn leaves_used_at_the_same_time_are_evicted_lowest_hash_first() {
    // Two tenants store 4 blocks each at t = 0: 8 > 5, so three leaves go, each
    // time the lower-hashed of the two chains' current leaves.
    let mut m = mock_with(&[profile_toml("b", "block_granular", &[])], 1);
    assert_eq!(cached(&mut m, &user("b", C), "x", 0), (0, 16));
    assert_eq!(cached(&mut m, &user("b", C), "y", 0), (0, 16));
    let bytes = format!("{{\"content\":\"{C}\",\"role\":\"user\"}}\n");
    let chain = |t: &str| -> Vec<[u8; 32]> {
        (1..=4)
            .map(|j| block_hash(t, &bytes.as_bytes()[..16 * j]))
            .collect()
    };
    let (mut x, mut y) = (chain("x"), chain("y"));
    for _ in 0..3 {
        if x.last() < y.last() || y.is_empty() {
            x.pop();
        } else {
            y.pop();
        }
    }
    assert_eq!(m.cache_sizes().1, 5);
    let blocks = |n: usize| 4 * n as u64;
    // Each read rewrites what it misses and may evict the other chain: read each
    // from the state both share.
    let mut other = m.clone();
    assert_eq!(cached(&mut other, &user("b", C), "y", 1).0, blocks(y.len()));
    assert_eq!(
        cached(&mut m, &user("b", C), "x", 1).0,
        blocks(x.len()),
        "x keeps {} blocks",
        x.len()
    );
}

/// Cites: MLM-20
#[test]
fn profiles_never_share_cache_entries() {
    let mut m = mock_with(
        &[
            profile_toml("a1", "automatic_prefix", &[]),
            profile_toml("a2", "automatic_prefix", &["ttl_ns = 1"]),
        ],
        1,
    );
    assert_eq!(cached(&mut m, &user("a1", C), "-", 0), (0, 0));
    assert_eq!(
        cached(&mut m, &user("a2", C), "-", 10),
        (0, 0),
        "another profile is another model"
    );
    assert_eq!(
        cached(&mut m, &user("a1", C), "-", 20),
        (16, 0),
        "a2's short ttl does not expire a1's entries"
    );
}

/// Cites: MLM-21, MLM-2
#[test]
fn uncached_read_and_written_tokens_partition_the_prompt_as_a_breakpoint_rolls() {
    // A growing conversation whose last message is marked, as `rolling_tail`
    // marks it: each request reads the previous marked prefix and writes a
    // longer one. The write is only what lies beyond the read (issue #12), so
    // the three counts never exceed the prompt.
    let mut m = mock_with(&[profile_toml("e", "explicit_breakpoints", &[])], 1);
    let s33 = "s".repeat(33);
    let turns = [
        "one two three four",
        "five six seven",
        "eight nine ten eleven twelve",
    ];
    for k in 1..=turns.len() {
        let users: Vec<(&str, bool)> = turns[..k].iter().map(|t| (*t, false)).collect();
        let mut users = users;
        if let Some(last) = users.last_mut() {
            last.1 = true;
        }
        let o = m.handle(&with_system(&s33, &users, "e"), "-", k as i64);
        assert_eq!(o.status, 200);
        let usage = &common::body(&o)["usage"];
        let prompt = usage["prompt_tokens"].as_u64().unwrap();
        let read = usage["prompt_tokens_details"]["cached_tokens"]
            .as_u64()
            .unwrap();
        let write = usage["prompt_tokens_details"]["cache_write_tokens"]
            .as_u64()
            .unwrap();
        assert!(
            read + write <= prompt,
            "turn {k}: {read} + {write} > {prompt}"
        );
        if k > 1 {
            assert!(
                read > 0 && write > 0,
                "turn {k}: reads the last prefix, writes the rest"
            );
        }
    }
}
