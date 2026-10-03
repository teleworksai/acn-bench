//! MLM-30, MLM-31: time to first token, token cadence, jitter bounds, queueing and
//! the timing header, on hand-computed values.

#![allow(clippy::unwrap_used, clippy::expect_used, clippy::panic)] // CON-19: tests are exempt

mod common;

use common::{header, mock_with, profile_toml, user};

const C: &str = "abcdefghijklmnopqrstuvwxyz012345678"; // 16 tokens

/// Cites: MLM-30, MLM-31
#[test]
fn ttft_is_queue_plus_prefill_and_tokens_follow_the_cadence() {
    // base 1000, 10/new token, 1/cached token, itl 100, no jitter, 5 tokens, 1 slot.
    let mut m = mock_with(&[profile_toml("t", "automatic_prefix", &["slots = 1"])], 1);
    let a = m.handle(&user("t", C), "-", 0);
    assert_eq!(
        a.token_times,
        vec![1160, 1260, 1360, 1460, 1560],
        "1000 + 10 x 16 new tokens"
    );
    assert_eq!(
        a.respond_at_ns, 1560,
        "the body is sent with the last token"
    );
    assert_eq!(
        header(&a, "x-acn-mock-timing"),
        Some("queue_ns=0 prefill_ns=1160 decode_ns=400")
    );
    // A second request arriving at once waits for the one slot, and finds the
    // prompt cached: 1000 + 1 x 16 cached tokens.
    let b = m.handle(&user("t", C), "-", 0);
    assert_eq!(b.timing.queue_ns, 1560);
    assert_eq!(b.token_times.first(), Some(&(1560 + 1016)));
    assert_eq!(
        header(&b, "x-acn-mock-timing"),
        Some("queue_ns=1560 prefill_ns=1016 decode_ns=400")
    );
    // With unlimited slots nobody queues.
    let mut m = mock_with(&[profile_toml("t", "automatic_prefix", &[])], 1);
    m.handle(&user("t", C), "-", 0);
    assert_eq!(m.handle(&user("t", C), "-", 0).timing.queue_ns, 0);
}

/// Cites: MLM-30, MLM-6
#[test]
fn jitter_stays_in_its_bounds_and_never_reorders_tokens() {
    let mut m = mock_with(
        &[profile_toml(
            "t",
            "automatic_prefix",
            &[
                "itl_jitter_ns = 30",
                "output_tokens_min = 200",
                "output_tokens_max = 200",
            ],
        )],
        7,
    );
    let o = m.handle(&user("t", C), "-", 0);
    assert_eq!(
        o.token_times[0], 1160,
        "the first token is at the time to first token"
    );
    let mut seen_jitter = false;
    for (k, w) in o.token_times.windows(2).enumerate() {
        assert!(w[1] >= w[0], "token {k}");
        let nominal = 1160 + 100 * i64::try_from(k + 1).unwrap();
        let off = w[1].max(nominal) - nominal.min(w[1]);
        assert!(off <= 30, "token {} is {off} ns off its slot", k + 1);
        seen_jitter |= w[1] != nominal;
    }
    assert!(seen_jitter, "jitter is drawn");
}

fn limited(model: &str, max_tokens: u64) -> Vec<u8> {
    serde_json::json!({ "model": model, "max_tokens": max_tokens,
        "messages": [{ "role": "user", "content": C }] })
    .to_string()
    .into_bytes()
}

/// Cites: MLM-30
#[test]
fn several_slots_serve_fifo_from_the_slot_that_frees_first() {
    // Nothing cacheable, so every prefill is 1000 + 10 x 16 = 1160.
    let mut m = mock_with(
        &[profile_toml(
            "t",
            "automatic_prefix",
            &["slots = 2", "min_cacheable_tokens = 1000"],
        )],
        1,
    );
    let a = m.handle(&limited("t", 1), "-", 0); // last token at 1160
    let b = m.handle(&limited("t", 5), "-", 0); // last token at 1560
    assert_eq!((a.timing.queue_ns, b.timing.queue_ns), (0, 0));
    assert_eq!((a.respond_at_ns, b.respond_at_ns), (1160, 1560));
    let c = m.handle(&limited("t", 5), "-", 0);
    assert_eq!(c.timing.queue_ns, 1160, "a's slot frees first");
    assert_eq!(c.respond_at_ns, 1160 + 1160 + 400);
    let d = m.handle(&limited("t", 1), "-", 0);
    assert_eq!(d.timing.queue_ns, 1560, "then b's");
    let e = m.handle(&limited("t", 1), "-", 5000);
    assert_eq!(e.timing.queue_ns, 0, "both free again");
}

/// Cites: MLM-30
#[test]
fn each_profile_has_its_own_slots() {
    let mut m = mock_with(
        &[
            profile_toml("four", "automatic_prefix", &["slots = 4"]),
            profile_toml("one", "automatic_prefix", &["slots = 1"]),
        ],
        1,
    );
    m.handle(&user("four", C), "-", 0);
    let a = m.handle(&user("one", C), "-", 0);
    let b = m.handle(&user("one", C), "-", 0);
    assert_eq!(a.timing.queue_ns, 0);
    assert_eq!(
        b.timing.queue_ns, a.respond_at_ns,
        "one slot, whatever another profile has"
    );
}

/// Cites: MLM-31
#[test]
fn the_timing_header_is_on_every_response() {
    let mut m = mock_with(
        &[
            profile_toml("t", "automatic_prefix", &[]),
            profile_toml("f", "automatic_prefix", &["fault_429_ppm = 1000000"]),
        ],
        1,
    );
    let streamed = serde_json::json!({ "model": "t", "stream": true,
        "messages": [{ "role": "user", "content": C }] })
    .to_string()
    .into_bytes();
    let s = m.handle(&streamed, "-", 0);
    assert_eq!(
        header(&s, "x-acn-mock-timing"),
        Some("queue_ns=0 prefill_ns=1160 decode_ns=400")
    );
    for (o, status) in [
        (m.handle(&user("f", C), "-", 0), 429),
        (m.handle(b"nope", "-", 0), 400),
        (m.handle(&user("missing", C), "-", 0), 400),
    ] {
        assert_eq!(o.status, status);
        assert_eq!(
            header(&o, "x-acn-mock-timing"),
            Some("queue_ns=0 prefill_ns=0 decode_ns=0"),
            "an error is answered at arrival"
        );
    }
}
