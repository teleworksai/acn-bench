//! TRC-21: provider fields are normalised by a recorded mapping, not by code that knows providers.

#![allow(clippy::unwrap_used, clippy::expect_used, clippy::panic)] // CON-19: tests are exempt

use acn_trace::normalise::{self, StopReason};
use acn_trace::schema;
use serde_json::json;

/// Cites: TRC-21, TRC-12
#[test]
fn anthropic_usage_excludes_cached_tokens_so_the_total_is_a_sum() {
    let inv = schema::inventory().expect("inventory");
    let raw = json!({
        "stop_reason": "tool_use",
        "usage": { "input_tokens": 12, "cache_read_input_tokens": 9000, "cache_creation_input_tokens": 500, "output_tokens": 77 }
    });
    let n = normalise::response(&inv, "anthropic", &raw).expect("normalise");
    assert_eq!(n.input_tokens, Some(9512));
    assert_eq!(n.cache_read_tokens, Some(9000));
    assert_eq!(n.cache_write_tokens, Some(500));
    assert_eq!(n.output_tokens, Some(77));
    assert_eq!(n.stop_reason, Some(StopReason::ToolUse));
    assert_eq!(n.stop_reason_raw.as_deref(), Some("tool_use"));
    // The mapping records where each number came from, so a verdict is auditable.
    assert_eq!(
        n.sources["cache_read_tokens"],
        "usage.cache_read_input_tokens"
    );
    assert_eq!(
        n.sources["input_tokens"],
        "usage.input_tokens + usage.cache_read_input_tokens + usage.cache_creation_input_tokens"
    );
}

/// Cites: TRC-21, TRC-12
#[test]
fn openai_prompt_tokens_already_include_cached_tokens_and_there_is_no_write_count() {
    let inv = schema::inventory().expect("inventory");
    let raw = json!({
        "choices": [{ "finish_reason": "stop" }],
        "usage": { "prompt_tokens": 9512, "completion_tokens": 77, "prompt_tokens_details": { "cached_tokens": 9000 } }
    });
    let n = normalise::response(&inv, "openai", &raw).expect("normalise");
    assert_eq!(n.input_tokens, Some(9512));
    assert_eq!(n.cache_read_tokens, Some(9000));
    assert_eq!(
        n.cache_write_tokens,
        Some(0),
        "0 when the provider has no such concept"
    );
    assert_eq!(
        n.stop_reason,
        Some(StopReason::EndTurn),
        "an OpenAI-compatible `stop` is `end_turn`"
    );
    assert_eq!(n.stop_reason_raw.as_deref(), Some("stop"));
}

/// Cites: TRC-21
#[test]
fn an_unknown_stop_value_becomes_other_and_the_raw_value_is_kept() {
    let inv = schema::inventory().expect("inventory");
    let raw = json!({ "stop_reason": "a_value_from_next_year", "usage": { "input_tokens": 1, "output_tokens": 1 } });
    let n = normalise::response(&inv, "anthropic", &raw).expect("normalise");
    assert_eq!(n.stop_reason, Some(StopReason::Other));
    assert_eq!(n.stop_reason_raw.as_deref(), Some("a_value_from_next_year"));
}

/// Cites: TRC-12
#[test]
fn a_response_without_usage_yields_absent_counts_never_zero() {
    let inv = schema::inventory().expect("inventory");
    let n = normalise::response(
        &inv,
        "openai",
        &json!({ "error": { "message": "upstream timeout" } }),
    )
    .expect("normalise");
    assert_eq!(n.input_tokens, None);
    assert_eq!(n.output_tokens, None);
    assert_eq!(n.cache_read_tokens, None);
    assert_eq!(
        n.cache_write_tokens, None,
        "no usage at all is absent, not the provider-has-no-concept zero"
    );
    assert_eq!(n.stop_reason, None);
}

/// Cites: TRC-21
#[test]
fn every_backend_the_spec_names_has_a_mapping_and_an_unknown_one_is_an_error() {
    let inv = schema::inventory().expect("inventory");
    for p in ["anthropic", "openai", "vllm", "sglang", "mockllm"] {
        let m = inv
            .provider(p)
            .unwrap_or_else(|| panic!("no mapping for {p}"));
        assert!(!m.stop_reason_field.is_empty(), "{p}");
        assert!(!m.input_tokens.is_empty(), "{p}");
    }
    let err = normalise::response(&inv, "acme", &json!({}))
        .expect_err("unknown provider")
        .to_string();
    assert!(err.contains("acme"), "{err}");
}

// ---- adversarial review of T02a: each case below produced a wrong or invented value.

/// Cites: TRC-12, TRC-21
#[test]
fn a_cache_count_the_server_does_not_report_is_absent_not_zero() {
    let inv = schema::inventory().expect("inventory");
    // vLLM without --enable-prompt-tokens-details, or with a zero count: the field is null or missing.
    for body in [
        json!({ "choices": [{ "finish_reason": "stop" }], "usage": { "prompt_tokens": 100, "completion_tokens": 5, "prompt_tokens_details": null } }),
        json!({ "choices": [{ "finish_reason": "stop" }], "usage": { "prompt_tokens": 100, "completion_tokens": 5 } }),
    ] {
        for p in ["vllm", "sglang", "openai"] {
            let n = normalise::response(&inv, p, &body).expect("normalise");
            assert_eq!(n.input_tokens, Some(100), "{p}");
            assert_eq!(n.cache_read_tokens, None, "{p}: unknown is not zero");
        }
    }
    // Anthropic always reports the field; a response that omits it had no cache read.
    let n = normalise::response(
        &inv,
        "anthropic",
        &json!({ "stop_reason": "end_turn", "usage": { "input_tokens": 7, "output_tokens": 1 } }),
    )
    .expect("normalise");
    assert_eq!(n.cache_read_tokens, Some(0));
    assert_eq!(n.cache_write_tokens, Some(0));
}

/// Cites: TRC-12, TRC-21
#[test]
fn a_server_side_abort_is_not_a_client_abort_and_the_newer_anthropic_values_are_mapped() {
    let inv = schema::inventory().expect("inventory");
    let vllm = json!({ "choices": [{ "finish_reason": "abort" }], "usage": { "prompt_tokens": 1, "completion_tokens": 0 } });
    assert_eq!(
        normalise::response(&inv, "vllm", &vllm)
            .expect("normalise")
            .stop_reason,
        Some(StopReason::Other)
    );
    for (raw, want) in [
        ("pause_turn", StopReason::Other),
        ("model_context_window_exceeded", StopReason::MaxTokens),
        ("refusal", StopReason::ContentFilter),
    ] {
        let body =
            json!({ "stop_reason": raw, "usage": { "input_tokens": 1, "output_tokens": 1 } });
        assert_eq!(
            normalise::response(&inv, "anthropic", &body)
                .expect("normalise")
                .stop_reason,
            Some(want),
            "{raw}"
        );
    }
}

/// Cites: TRC-21
#[test]
fn a_body_the_mapping_does_not_describe_is_an_error_not_silence() {
    let inv = schema::inventory().expect("inventory");
    // An OpenAI Responses API body: a usage object with other field names.
    let responses_api = json!({ "status": "completed", "usage": { "input_tokens": 50, "output_tokens": 5, "input_tokens_details": { "cached_tokens": 40 } } });
    let err = normalise::response(&inv, "openai", &responses_api)
        .expect_err("unmapped wire format")
        .to_string();
    assert!(err.contains("usage.prompt_tokens"), "{err}");
    for (body, needle) in [
        (
            json!({ "usage": { "prompt_tokens": 9_223_372_036_854_775_808_u64, "completion_tokens": 1 } }),
            "64-bit",
        ),
        (
            json!({ "usage": { "prompt_tokens": 10, "completion_tokens": 1, "prompt_tokens_details": { "cached_tokens": 9000 } } }),
            "exceeds",
        ),
        (
            json!({ "usage": { "prompt_tokens": -3, "completion_tokens": 1 } }),
            "non-negative",
        ),
        (
            json!({ "usage": { "prompt_tokens": "12", "completion_tokens": 1 } }),
            "non-negative",
        ),
    ] {
        let err = normalise::response(&inv, "openai", &body)
            .expect_err("must be rejected")
            .to_string();
        assert!(err.contains(needle), "expected `{needle}` in: {err}");
    }
    let big = i64::MAX as u64;
    let overflow = json!({ "stop_reason": "end_turn", "usage": { "input_tokens": big, "cache_read_input_tokens": big, "output_tokens": 1 } });
    assert!(
        normalise::response(&inv, "anthropic", &overflow).is_err(),
        "a sum that overflows is an error, never saturated"
    );
}

/// Cites: TRC-12
#[test]
fn without_usage_the_output_count_is_absent_too() {
    let inv = schema::inventory().expect("inventory");
    let n = normalise::response(&inv, "anthropic", &json!({ "stop_reason": "end_turn" }))
        .expect("normalise");
    assert_eq!(
        (
            n.input_tokens,
            n.output_tokens,
            n.cache_read_tokens,
            n.cache_write_tokens
        ),
        (None, None, None, None)
    );
    assert_eq!(n.stop_reason, Some(StopReason::EndTurn));
}
