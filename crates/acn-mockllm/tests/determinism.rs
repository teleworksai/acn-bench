//! MLM-5..8: the library and the HTTP server agree; two runs are byte-identical;
//! the order concurrent requests are delivered in does not matter.

#![allow(clippy::unwrap_used, clippy::expect_used, clippy::panic)] // CON-19: tests are exempt

mod common;

use std::sync::Arc;

use acn_emu::clock::SimClock;
use acn_mockllm::Mock;
use common::user;
use serde_json::json;

const C: &str = "abcdefghijklmnopqrstuvwxyz012345678";

fn sequence() -> Vec<(Vec<u8>, String, i64)> {
    let streamed = json!({ "model": "mock-auto", "stream": true, "stream_options": { "include_usage": true },
        "messages": [{ "role": "user", "content": C }] });
    vec![
        (user("mock-auto", C), "-".into(), 0),
        (user("mock-blocks", C), "-".into(), 5),
        (streamed.to_string().into_bytes(), "-".into(), 9),
        (user("mock-explicit", "hi"), "t2".into(), 9),
    ]
}

/// Cites: MLM-7, MLM-6
#[test]
fn two_runs_with_the_same_seed_are_identical_and_seeds_matter() {
    let run = |seed| {
        let mut m = Mock::new(seed).unwrap();
        sequence()
            .iter()
            .map(|(b, t, at)| m.handle(b, t, *at))
            .collect::<Vec<_>>()
    };
    assert_eq!(run(42), run(42));
    let (a, b) = (run(42), run(43));
    assert_ne!(a[0].body, b[0].body, "another seed draws other words");
    assert_eq!(
        a[0].accounting, b[0].accounting,
        "but caching does not depend on the seed"
    );
}

/// Cites: MLM-7
#[test]
fn delivery_order_does_not_change_any_result() {
    let mut reqs = sequence();
    // Two requests at the same instant, given in both orders.
    reqs.push((user("mock-auto", "zzzz"), "-".into(), 9));
    let a = Mock::new(1).unwrap().handle_batch(&reqs);
    let mut reversed = reqs.clone();
    reversed.reverse();
    let mut b = Mock::new(1).unwrap().handle_batch(&reversed);
    b.reverse();
    assert_eq!(a, b);
}

/// Cites: MLM-8
#[test]
fn every_instance_starts_with_an_empty_cache() {
    let mut m = Mock::new(1).unwrap();
    assert_eq!(m.cache_sizes(), (0, 0));
    m.handle(&user("mock-auto", &C.repeat(200)), "-", 0); // about 1757 tokens, above the 1024 minimum
    assert!(m.cache_sizes().0 > 0);
    assert_eq!(Mock::new(1).unwrap().cache_sizes(), (0, 0));
}

/// Cites: MLM-5, MLM-4
#[tokio::test]
async fn the_http_server_answers_exactly_as_the_library_does() {
    let clock = Arc::new(SimClock::new());
    let listener = tokio::net::TcpListener::bind("127.0.0.1:0").await.unwrap();
    let addr = listener.local_addr().unwrap();
    let server = tokio::spawn(acn_mockllm::server::serve(
        listener,
        Mock::new(77).unwrap(),
        clock.clone(),
    ));
    let client = reqwest::Client::new();
    let mut lib = Mock::new(77).unwrap();
    let mut at = 0i64;
    for (body, _, _) in sequence().into_iter().take(3) {
        let resp = client
            .post(format!("http://{addr}/v1/chat/completions"))
            .header("content-type", "application/json")
            .body(body.clone())
            .send()
            .await
            .unwrap();
        assert!(resp.headers().get("x-acn-mockllm").is_some());
        let text = resp.text().await.unwrap();
        // The server read its clock on arrival; the library is given that time.
        let o = lib.handle(&body, "-", at);
        let expected = if o.stream {
            o.chunks
                .iter()
                .map(|c| format!("data: {}\n\n", c.data))
                .collect::<String>()
        } else {
            String::from_utf8(o.body.clone()).unwrap()
        };
        assert_eq!(text, expected);
        at = o.respond_at_ns;
        use acn_emu::clock::Clock as _;
        assert_eq!(
            clock.now_ns(),
            at,
            "the server waited on the sim clock until the last token"
        );
    }
    let models: serde_json::Value = client
        .get(format!("http://{addr}/v1/models"))
        .send()
        .await
        .unwrap()
        .json()
        .await
        .unwrap();
    assert_eq!(models["data"].as_array().unwrap().len(), 3);
    assert_eq!(
        models["profiles_blake3"],
        acn_mockllm::profile::embedded().unwrap().blake3
    );
    server.abort();
}
