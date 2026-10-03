//! MLM-1..4: request and response shapes, the stream assembling to the plain
//! response, the frozen `mockllm` mapping normalising a response, and the mock's
//! identity on everything it sends.

#![allow(clippy::unwrap_used, clippy::expect_used, clippy::panic)] // CON-19: tests are exempt

mod common;

use acn_trace::normalise::{self, StopReason};
use common::{body, header, mock_with, profile_toml};
use serde_json::{Value, json};

fn req(stream: bool, usage: bool, tools: bool) -> Vec<u8> {
    let mut r = json!({
        "model": "w", "stream": stream,
        "messages": [
            { "role": "system", "content": "be brief", "cache_control": { "type": "ephemeral" } },
            { "role": "user", "content": "abcdefghijklmnopqrstuvwxyz012345678" }
        ],
        "temperature": 0, "n": 1
    });
    if usage {
        r["stream_options"] = json!({ "include_usage": true });
    }
    if tools {
        r["tools"] =
            json!([{ "type": "function", "function": { "name": "read", "parameters": {} } }]);
    }
    r.to_string().into_bytes()
}

fn mock() -> acn_mockllm::Mock {
    mock_with(&[profile_toml("w", "automatic_prefix", &[])], 9)
}

/// Assemble a stream as TRC-21 states: deltas accumulated, usage from the last chunk.
fn assemble(o: &acn_mockllm::Outcome) -> Value {
    let mut role = Value::Null;
    let mut content: Option<String> = None;
    let mut tool_calls = Vec::new();
    let mut finish = Value::Null;
    let mut usage = Value::Null;
    for c in &o.chunks {
        if c.data == "[DONE]" {
            continue;
        }
        let v: Value = serde_json::from_str(&c.data).unwrap();
        assert_eq!(
            v["system_fingerprint"], "acn-mockllm:w",
            "every chunk says what it is"
        );
        if let Some(u) = v.get("usage") {
            usage = u.clone();
        }
        if let Some(ch) = v["choices"].get(0) {
            if let Some(r) = ch["delta"].get("role") {
                role = r.clone();
            }
            if let Some(t) = ch["delta"]["content"].as_str() {
                content.get_or_insert_default().push_str(t);
            }
            if let Some(tc) = ch["delta"]["tool_calls"].as_array() {
                for t in tc {
                    let mut t = t.clone();
                    t.as_object_mut().unwrap().remove("index");
                    tool_calls.push(t);
                }
            }
            if !ch["finish_reason"].is_null() {
                finish = ch["finish_reason"].clone();
            }
        }
    }
    let mut message = json!({ "role": role, "content": content });
    if !tool_calls.is_empty() {
        message["tool_calls"] = json!(tool_calls);
    }
    json!({ "choices": [{ "message": message, "finish_reason": finish }], "usage": usage })
}

/// Cites: MLM-1, MLM-2, TRC-21
#[test]
fn a_response_has_the_shape_the_frozen_mapping_reads() {
    let mut m = mock();
    m.handle(&req(false, false, false), "-", 0);
    let o = m.handle(&req(false, false, false), "-", 1);
    let b = body(&o);
    assert_eq!(b["object"], "chat.completion");
    assert!(b["id"].as_str().unwrap().starts_with("chatcmpl-"));
    assert_eq!(b["model"], "w");
    for k in ["prompt_tokens", "completion_tokens", "total_tokens"] {
        assert!(b["usage"][k].is_u64(), "{k}");
    }
    assert_eq!(
        b["usage"]["total_tokens"].as_u64(),
        Some(
            b["usage"]["prompt_tokens"].as_u64().unwrap()
                + b["usage"]["completion_tokens"].as_u64().unwrap()
        )
    );
    assert_eq!(b["choices"].as_array().unwrap().len(), 1, "one choice");
    assert!(
        b["usage"]["prompt_tokens_details"]["cache_write_tokens"].is_u64(),
        "always present"
    );
    let inv = acn_trace::schema::inventory().unwrap();
    let n = normalise::response(&inv, "mockllm", &b).unwrap();
    assert_eq!(n.input_tokens, b["usage"]["prompt_tokens"].as_u64());
    assert_eq!(
        n.cache_read_tokens,
        b["usage"]["prompt_tokens_details"]["cached_tokens"].as_u64()
    );
    assert!(
        n.cache_read_tokens.unwrap() > 0,
        "the second identical request reads the cache"
    );
    assert_eq!(n.cache_write_tokens, Some(0));
    assert_eq!(n.output_tokens, b["usage"]["completion_tokens"].as_u64());
    assert_eq!(n.stop_reason, Some(StopReason::EndTurn));
    let t = body(&m.handle(&req(false, false, true), "-", 2));
    assert_eq!(
        normalise::response(&inv, "mockllm", &t)
            .unwrap()
            .stop_reason,
        Some(StopReason::ToolUse)
    );
}

/// Cites: MLM-3
#[test]
fn a_stream_assembles_to_the_plain_response() {
    for tools in [false, true] {
        let plain = body(&mock().handle(&req(false, false, tools), "-", 0));
        let streamed = mock().handle(&req(true, true, tools), "-", 0);
        assert_eq!(header(&streamed, "content-type"), Some("text/event-stream"));
        assert_eq!(streamed.chunks.last().unwrap().data, "[DONE]");
        let a = assemble(&streamed);
        assert_eq!(
            a["choices"][0]["message"], plain["choices"][0]["message"],
            "tools {tools}"
        );
        assert_eq!(
            a["choices"][0]["finish_reason"],
            plain["choices"][0]["finish_reason"]
        );
        assert_eq!(a["usage"], plain["usage"]);
        // The token chunks are emitted at the token times.
        let times: Vec<i64> = streamed.chunks.iter().map(|c| c.at_ns).collect();
        assert!(times.windows(2).all(|w| w[0] <= w[1]));
        assert_eq!(*times.last().unwrap(), streamed.respond_at_ns);
    }
    // In a warm state too: two clones of one mock after the same request.
    let mut warm = mock();
    warm.handle(&req(false, false, false), "-", 0);
    let plain = body(&warm.clone().handle(&req(false, false, false), "-", 1));
    let streamed = warm.handle(&req(true, true, false), "-", 1);
    assert!(plain["usage"]["prompt_tokens_details"]["cached_tokens"].as_u64() > Some(0));
    assert_eq!(assemble(&streamed)["usage"], plain["usage"]);
    assert_eq!(
        assemble(&streamed)["choices"][0]["message"],
        plain["choices"][0]["message"]
    );
    // One chunk per token, the final chunk, the usage chunk, then [DONE].
    let n = plain["usage"]["completion_tokens"].as_u64().unwrap() as usize;
    assert_eq!(streamed.chunks.len(), n + 3);
    let at = |i: usize| -> Value { serde_json::from_str(&streamed.chunks[i].data).unwrap() };
    assert_eq!(at(0)["choices"][0]["delta"]["role"], "assistant");
    assert_eq!(at(n)["choices"][0]["finish_reason"], "stop");
    assert_eq!(at(n + 1)["choices"], json!([]));
    assert!(at(n + 1)["usage"].is_object());
    // Without include_usage there is no usage chunk.
    let o = mock().handle(&req(true, false, false), "-", 0);
    assert!(o.chunks.iter().all(|c| !c.data.contains("\"usage\"")));
}

/// Cites: MLM-4, CON-26
#[test]
fn every_response_says_it_is_the_mock() {
    let mut m = mock();
    let o = m.handle(&req(false, false, false), "-", 0);
    assert_eq!(
        header(&o, "x-acn-mockllm"),
        Some(format!("acn-mockllm/{} profile=w", env!("CARGO_PKG_VERSION")).as_str())
    );
    assert_eq!(body(&o)["system_fingerprint"], "acn-mockllm:w");
    let e = m.handle(b"not json", "-", 1);
    assert!(header(&e, "x-acn-mockllm").is_some(), "errors too");
}

/// Cites: MLM-1
#[test]
fn bad_requests_get_an_openai_shaped_400() {
    let mut m = mock();
    for bad in [
        &b"[]"[..],
        b"{\"model\":\"nope\",\"messages\":[]}",
        b"{\"model\":\"w\"}",
    ] {
        let o = m.handle(bad, "-", 0);
        assert_eq!(o.status, 400, "{}", String::from_utf8_lossy(bad));
        assert_eq!(body(&o)["error"]["type"], "invalid_request_error");
    }
}

fn with_limits(stream: bool, limits: Value) -> Vec<u8> {
    let mut r = json!({ "model": "w", "stream": stream,
        "messages": [{ "role": "user", "content": "x" }] });
    for (k, v) in limits.as_object().unwrap() {
        r[k] = v.clone();
    }
    r.to_string().into_bytes()
}

fn completion(limits: Value) -> (u64, Value) {
    let b = body(&mock().handle(&with_limits(false, limits), "-", 0));
    (
        b["usage"]["completion_tokens"].as_u64().unwrap(),
        b["choices"][0]["finish_reason"].clone(),
    )
}

/// Cites: MLM-1, MLM-40
#[test]
fn the_token_limit_is_max_completion_tokens_then_max_tokens_and_null_is_unset() {
    // The profile answers 5 tokens.
    assert_eq!(completion(json!({})), (5, json!("stop")));
    assert_eq!(completion(json!({ "max_tokens": 2 })), (2, json!("length")));
    assert_eq!(
        completion(json!({ "max_completion_tokens": 3 })),
        (3, json!("length"))
    );
    assert_eq!(
        completion(json!({ "max_completion_tokens": 3, "max_tokens": 1 })),
        (3, json!("length")),
        "max_completion_tokens wins"
    );
    assert_eq!(
        completion(json!({ "max_completion_tokens": null, "max_tokens": 2 })),
        (2, json!("length")),
        "a null is an unset field"
    );
    for bad in [json!(-1), json!(1.5), json!("4")] {
        let o = mock().handle(&with_limits(false, json!({ "max_tokens": bad })), "-", 0);
        assert_eq!(o.status, 400, "max_tokens = {bad}");
    }
}

/// Cites: MLM-1
#[test]
fn other_fields_are_ignored() {
    let plain = json!({ "model": "w", "messages": [{ "role": "user", "content": "x" }] });
    let extra = json!({ "temperature": 0.7, "n": 1, "user": "u", "top_p": 1,
        "messages": [{ "content": "x", "role": "user" }], "model": "w" });
    assert_eq!(
        mock().handle(plain.to_string().as_bytes(), "-", 0),
        mock().handle(extra.to_string().as_bytes(), "-", 0)
    );
}

/// Cites: MLM-3
#[test]
fn an_empty_answer_streams_to_the_same_empty_content() {
    let plain = body(&mock().handle(&with_limits(false, json!({ "max_tokens": 0 })), "-", 0));
    assert_eq!(plain["choices"][0]["message"]["content"], "");
    let streamed = mock().handle(
        &with_limits(
            true,
            json!({ "max_tokens": 0, "stream_options": { "include_usage": true } }),
        ),
        "-",
        0,
    );
    let a = assemble(&streamed);
    assert_eq!(a["choices"][0]["message"], plain["choices"][0]["message"]);
    assert_eq!(a["choices"][0]["finish_reason"], "length");
    assert_eq!(a["usage"], plain["usage"]);
}
