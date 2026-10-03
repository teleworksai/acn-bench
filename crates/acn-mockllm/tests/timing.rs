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
