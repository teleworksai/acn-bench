//! HAR-20..25: the two dialects on a golden context, which backends a build can
//! reach, credentials, the backend-identity probe, retries and timeouts against
//! the mock's faults, and the run options.

#![allow(clippy::unwrap_used, clippy::expect_used, clippy::panic)] // CON-19: tests are exempt

mod common;

use acn_harness::HarnessError;
use acn_harness::agent::Opts;
use acn_harness::context::{Context, Dialect, Msg, Sampling, ToolCall, ToolDef};
use acn_harness::knobs::Placement;
use acn_harness::wire::{Backend, Exchange};
use acn_trace::identity::Mode;
use common::{Spec, children, int, profile, profiles, run_fixture, session, spans, text};
use serde_json::json;

fn golden() -> Context {
    Context {
        system: "S".into(),
        tools: vec![ToolDef {
            name: "f".into(),
            description: "d".into(),
            parameters: json!({ "type": "object" }),
        }],
        messages: vec![
            Msg::User { text: "u".into() },
            Msg::Assistant {
                text: None,
                tool_calls: vec![ToolCall {
                    id: "c1".into(),
                    name: "f".into(),
                    arguments: "{}".into(),
                }],
            },
            Msg::ToolResult {
                call_id: "c1".into(),
                tool: "f".into(),
                ordinal: Some(0),
                content: "r".into(),
            },
            Msg::User { text: "v".into() },
        ],
    }
}

const SAMPLING: Sampling = Sampling {
    max_tokens: 64,
    temperature: 0.0,
    stream: true,
};

/// Cites: HAR-21
#[test]
fn both_dialects_encode_a_golden_context_deterministically() {
    let cc = golden().encode(
        Dialect::ChatCompletions,
        "m",
        SAMPLING,
        Placement::RollingTail,
        true,
    );
    assert_eq!(
        cc.to_string(),
        concat!(
            r#"{"max_completion_tokens":64,"messages":["#,
            r#"{"content":[{"cache_control":{"type":"ephemeral"},"text":"S","type":"text"}],"role":"system"},"#,
            r#"{"content":"u","role":"user"},"#,
            r#"{"content":null,"role":"assistant","tool_calls":[{"function":{"arguments":"{}","name":"f"},"id":"c1","type":"function"}]},"#,
            r#"{"content":"r","role":"tool","tool_call_id":"c1"},"#,
            r#"{"cache_control":{"type":"ephemeral"},"content":"v","role":"user"}],"#,
            r#""model":"m","stream":true,"stream_options":{"include_usage":true},"temperature":0.0,"#,
            r#""tools":[{"function":{"description":"d","name":"f","parameters":{"type":"object"}},"type":"function"}]}"#,
        ),
        "keys sorted, no whitespace: the bytes are a function of the context"
    );
    let msgs = golden().encode(
        Dialect::Messages,
        "m",
        SAMPLING,
        Placement::SystemAndTools,
        true,
    );
    assert_eq!(
        msgs,
        json!({
            "model": "m", "max_tokens": 64, "temperature": 0.0, "stream": true,
            "system": [{ "type": "text", "text": "S", "cache_control": { "type": "ephemeral" } }],
            "tools": [{ "name": "f", "description": "d", "input_schema": { "type": "object" },
                        "cache_control": { "type": "ephemeral" } }],
            "messages": [
                { "role": "user", "content": [{ "type": "text", "text": "u" }] },
                { "role": "assistant", "content": [{ "type": "tool_use", "id": "c1", "name": "f", "input": {} }] },
                // The tool result and the next user turn share one user message.
                { "role": "user", "content": [
                    { "type": "tool_result", "tool_use_id": "c1", "content": "r" },
                    { "type": "text", "text": "v" }
                ] }
            ]
        })
    );
}

/// Cites: HAR-20, HAR-52
#[test]
fn the_mock_runs_in_process_and_nothing_else_runs_in_sim() {
    for b in ["openai", "vllm", "sglang", "anthropic", "mockllm"] {
        assert_eq!(Backend::parse(b).unwrap().as_str(), b);
    }
    assert!(Backend::parse("bedrock").is_err());
    assert_eq!(Backend::Anthropic.dialect(), Dialect::Messages);
    assert_eq!(Backend::Vllm.dialect(), Dialect::ChatCompletions);
    let mut f = run_fixture(&common::smoke(), "auto");
    f.cfg.backend = Backend::Openai;
    let err = acn_harness::run::run(&f.cfg).unwrap_err();
    assert!(
        err.to_string()
            .contains("only the mockllm backend runs in sim"),
        "{err}"
    );
    f.cfg.mode = Mode::Netem;
    assert!(acn_harness::run::run(&f.cfg).is_err());
    #[cfg(not(feature = "real-api"))]
    {
        f.cfg.mode = Mode::Live;
        f.cfg.opts.endpoint = "http://127.0.0.1:9".into();
        let err = acn_harness::run::run(&f.cfg).unwrap_err();
        assert!(err.to_string().contains("real-api"), "{err}");
    }
}

/// Cites: HAR-22
#[test]
fn credentials_come_from_the_environment_and_go_only_into_the_auth_header() {
    use acn_harness::credentials::headers_with;
    let secret = "sk-test-0123456789";
    let env =
        |n: &str| (n == "OPENAI_API_KEY" || n == "ANTHROPIC_API_KEY").then(|| secret.to_owned());
    let h = headers_with(Backend::Openai, env).unwrap();
    assert_eq!(
        h,
        [("authorization".to_owned(), format!("Bearer {secret}"))]
    );
    let a = headers_with(Backend::Anthropic, env).unwrap();
    assert!(a.contains(&("x-api-key".to_owned(), secret.to_owned())));
    assert!(a.iter().any(|(k, _)| k == "anthropic-version"));
    assert!(
        headers_with(Backend::Vllm, |_| None).unwrap().is_empty(),
        "optional"
    );
    let err = headers_with(Backend::Openai, |_| None)
        .unwrap_err()
        .to_string();
    assert!(
        err.contains("OPENAI_API_KEY"),
        "the variable is named: {err}"
    );
    // A live environment never prints its headers.
    let live = acn_harness::env::LiveEnv::new(
        std::sync::Arc::new(acn_emu::clock::WallClock::start()),
        "http://127.0.0.1:9",
        h,
    )
    .unwrap();
    assert!(!format!("{live:?}").contains(secret));
    // Endpoints may not smuggle credentials either.
    let mut f = run_fixture(&common::smoke(), "auto");
    f.cfg.mode = Mode::Live;
    f.cfg.opts.endpoint = format!("http://user:{secret}@127.0.0.1:9");
    let err = acn_harness::run::run(&f.cfg).unwrap_err().to_string();
    assert!(
        err.contains("carries credentials") && !err.contains(secret),
        "{err}"
    );
}

/// Cites: HAR-23, CON-26, MLM-4
#[test]
fn a_live_run_refuses_an_endpoint_that_is_not_what_it_was_configured_as() {
    // Configured as the mock, pointed at something else: refused, nothing written.
    let mut f = run_fixture(&common::smoke(), "auto");
    f.cfg.mode = Mode::Live;
    f.cfg.opts.endpoint = common::plain_server();
    let err = acn_harness::run::run(&f.cfg).unwrap_err();
    assert!(matches!(err, HarnessError::BackendMismatch(_)), "{err}");
    assert!(err.to_string().starts_with("backend_mismatch"));
    assert!(
        !f.cfg.runs_dir.exists() || std::fs::read_dir(&f.cfg.runs_dir).unwrap().next().is_none()
    );
    // Every response is inspected for either marker (MLM-4).
    let with_header = Exchange {
        headers: vec![("x-acn-mockllm".into(), "acn-mockllm/0.1.0 profile=x".into())],
        ..Exchange::default()
    };
    assert!(with_header.from_mock());
    let with_fingerprint = Exchange {
        events: vec![(
            0,
            r#"{"system_fingerprint":"acn-mockllm:x","choices":[]}"#.into(),
        )],
        ..Exchange::default()
    };
    assert!(with_fingerprint.from_mock());
    let plain = Exchange {
        body: br#"{"system_fingerprint":"fp_123"}"#.to_vec(),
        ..Exchange::default()
    };
    assert!(!plain.from_mock());
}

fn failing(fault: &str, opts: Opts) -> common::Ran {
    session(Spec {
        profiles: profiles(&[profile("auto", "automatic_prefix", &[fault])]),
        opts,
        ..Spec::default()
    })
}

fn retry_opts() -> Opts {
    Opts {
        max_retries: 2,
        retry_base_ms: 500,
        ..Opts::default()
    }
}

/// Cites: HAR-24, HAR-1, MLM-41
#[test]
fn retries_wait_as_told_count_and_end_the_turn_aborted() {
    // 500s carry no retry-after: 500 ms, then 1000 ms, then give up.
    let r = failing("fault_500_ppm = 1000000", retry_opts());
    r.result.unwrap();
    let chat = spans(&r.trace, "chat")[0];
    assert_eq!(int(chat, "acn.call.retries"), Some(2));
    assert_eq!(text(chat, "acn.call.stop_reason"), Some("transport_error"));
    assert_eq!(text(chat, "acn.call.error_class"), Some("http_500"));
    assert_eq!(chat.end_ns - chat.start_ns, 1_500_000_000);
    assert_eq!(
        r.bodies.len(),
        3 * 3,
        "three attempts in each of three turns"
    );
    let turn = spans(&r.trace, "acn.turn")[0];
    assert_eq!(text(turn, "acn.turn.outcome"), Some("aborted"));
    assert!(
        children(&r.trace, turn)
            .iter()
            .all(|s| s.name != "execute_tool")
    );
    assert!(
        int(chat, "acn.call.input_tokens").is_none(),
        "no usage, no count"
    );
    // 429s say how long to wait: whole seconds from the response.
    let r = failing("fault_429_ppm = 1000000", retry_opts());
    let chat = spans(&r.trace, "chat")[0];
    assert_eq!(text(chat, "acn.call.error_class"), Some("http_429"));
    let waited = chat.end_ns - chat.start_ns;
    assert!(
        waited % 1_000_000_000 == 0 && (2_000_000_000..=4_000_000_000).contains(&waited),
        "{waited}"
    );
    // A stream cut before its final chunk is a transport error, and is retried.
    let r = failing("fault_cut_ppm = 1000000", retry_opts());
    let chat = spans(&r.trace, "chat")[0];
    assert_eq!(int(chat, "acn.call.retries"), Some(2));
    assert!(
        text(chat, "acn.call.error_class")
            .unwrap()
            .starts_with("transport: the stream ended")
    );
}

/// Cites: HAR-24
#[test]
fn an_attempt_past_its_timeout_is_abandoned_as_client_abort() {
    let r = failing(
        "fault_cut_ppm = 0",
        Opts {
            request_timeout_ms: 1,
            ..retry_opts()
        },
    );
    let chat = spans(&r.trace, "chat")[0];
    assert_eq!(text(chat, "acn.call.stop_reason"), Some("client_abort"));
    assert_eq!(text(chat, "acn.call.error_class"), Some("timeout"));
    assert_eq!(
        int(chat, "acn.call.retries"),
        Some(0),
        "a timeout is not retried"
    );
    assert_eq!(chat.end_ns - chat.start_ns, 1_000_000);
}

/// Cites: HAR-25, CON-29
#[test]
fn the_options_are_run_parameters_recorded_with_their_defaults() {
    let inv = acn_trace::schema::inventory().unwrap();
    let d = Opts::default();
    for (name, default, attr) in [
        ("opt.endpoint", String::new(), "acn.harness.endpoint"),
        (
            "opt.max_retries",
            d.max_retries.to_string(),
            "acn.harness.max_retries",
        ),
        (
            "opt.retry_base_ms",
            "500.0".into(),
            "acn.harness.retry_base_ms",
        ),
        (
            "opt.request_timeout_ms",
            "600000.0".into(),
            "acn.harness.request_timeout_ms",
        ),
        (
            "opt.stall_threshold_ms",
            "250.0".into(),
            "acn.stall_threshold_ms",
        ),
    ] {
        let o = inv.option(name).unwrap();
        assert_eq!(o.default, default, "{name}");
        assert_eq!(o.attribute, attr);
    }
    // A default stays out of the identity; anything else enters it (CON-29).
    let f = run_fixture(&common::smoke(), "auto");
    let w = acn_harness::run::run(&f.cfg).unwrap();
    let manifest: serde_json::Value =
        serde_json::from_slice(&std::fs::read(w.dir.join("manifest.json")).unwrap()).unwrap();
    assert!(
        manifest["params"]
            .as_object()
            .unwrap()
            .keys()
            .all(|k| !k.starts_with("opt."))
    );
    let mut g = run_fixture(&common::smoke(), "auto");
    g.cfg.opts.max_retries = 5;
    let w2 = acn_harness::run::run(&g.cfg).unwrap();
    let m2: serde_json::Value =
        serde_json::from_slice(&std::fs::read(w2.dir.join("manifest.json")).unwrap()).unwrap();
    assert_eq!(m2["params"]["opt.max_retries"], "5");
    assert_ne!(w.run_id, w2.run_id);
    let session = &common::read(&w2.dir)
        .spans
        .into_iter()
        .find(|s| s.name == "acn.session")
        .unwrap();
    assert_eq!(int(session, "acn.harness.max_retries"), Some(5));
}
