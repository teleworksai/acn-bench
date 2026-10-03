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
    // The final state too: a further request answers alike.
    let state = |seed| {
        let mut m = Mock::new(seed).unwrap();
        for (b, t, at) in sequence() {
            m.handle(&b, &t, at);
        }
        (m.cache_sizes(), m.handle(&user("mock-blocks", C), "-", 100))
    };
    assert_eq!(state(42), state(42));
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

/// Cites: MLM-7
#[test]
fn simultaneous_requests_go_by_tenant_then_prompt_hash() {
    let profiles = acn_mockllm::profile::embedded().unwrap();
    let prompt_hash = |b: &[u8]| {
        let v: serde_json::Value = serde_json::from_slice(b).unwrap();
        let p = profiles.get(v["model"].as_str().unwrap()).unwrap();
        *blake3::hash(&acn_mockllm::prompt::prompt(&v, p).unwrap().bytes).as_bytes()
    };
    let reqs: Vec<(Vec<u8>, String, i64)> = vec![
        (user("mock-auto", "q1"), "b".into(), 3),
        (user("mock-auto", "q2"), "b".into(), 3),
        (user("mock-auto", "q3"), "a".into(), 3),
        (user("mock-auto", "q4"), "a".into(), 3),
        (user("mock-auto", "q0"), "z".into(), 2),
    ];
    let mut expected_order: Vec<usize> = (0..reqs.len()).collect();
    expected_order.sort_by_key(|&i| (reqs[i].2, reqs[i].1.clone(), prompt_hash(&reqs[i].0)));
    let mut m = Mock::new(4).unwrap();
    let mut expected = vec![None; reqs.len()];
    for i in expected_order {
        expected[i] = Some(m.handle(&reqs[i].0, &reqs[i].1, reqs[i].2));
    }
    let expected: Vec<_> = expected.into_iter().flatten().collect();
    assert_eq!(Mock::new(4).unwrap().handle_batch(&reqs), expected);
    // Spelling a body differently, or adding an ignored field, changes no result.
    let mut respelled = reqs.clone();
    respelled[0].0 = json!({ "messages": [{ "content": "q1", "role": "user" }], "temperature": 1, "model": "mock-auto" })
        .to_string()
        .into_bytes();
    assert_eq!(Mock::new(4).unwrap().handle_batch(&respelled), expected);
}

/// Cites: MLM-6
#[test]
fn the_draws_come_from_the_mockllm_sub_stream() {
    use rand_core::Rng as _;
    // A 50% 429 rate: the first draw of each seed's `mockllm` stream decides.
    let profile = common::profile_toml("f", "automatic_prefix", &["fault_429_ppm = 500000"]);
    for seed in 0..40u64 {
        let mut rng = acn_trace::identity::substream_rng(seed, "mockllm").unwrap();
        let expect_429 = rng.next_u64() % 1_000_000 < 500_000;
        let mut m = common::mock_with(std::slice::from_ref(&profile), seed);
        assert_eq!(
            m.handle(&user("f", "x"), "-", 0).status == 429,
            expect_429,
            "seed {seed}"
        );
    }
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
    let mut requests = sequence();
    requests.push((
        b"{\"model\":\"nope\",\"messages\":[]}".to_vec(),
        "t3".into(),
        0,
    ));
    for (body, tenant, _) in requests {
        let mut post = client
            .post(format!("http://{addr}/v1/chat/completions"))
            .header("content-type", "application/json")
            .body(body.clone());
        if tenant != "-" {
            post = post.header("authorization", &tenant);
        }
        let resp = post.send().await.unwrap();
        // The server read its clock on arrival; the library is given that time.
        let o = lib.handle(&body, &tenant, at);
        assert_eq!(resp.status().as_u16(), o.status);
        for name in ["x-acn-mockllm", "x-acn-mock-timing", "content-type"] {
            assert_eq!(
                resp.headers().get(name).and_then(|v| v.to_str().ok()),
                common::header(&o, name),
                "{name}"
            );
        }
        let text = resp.text().await.unwrap();
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
    // What the router answers itself is marked and OpenAI-shaped too (MLM-1, MLM-4).
    let marker = acn_mockllm::engine::marker("-");
    for resp in [
        client
            .get(format!("http://{addr}/v1/nothing"))
            .send()
            .await
            .unwrap(),
        client
            .get(format!("http://{addr}/v1/chat/completions"))
            .send()
            .await
            .unwrap(),
        client
            .get(format!("http://{addr}/v1/models"))
            .send()
            .await
            .unwrap(),
    ] {
        assert_eq!(
            resp.headers()
                .get("x-acn-mockllm")
                .and_then(|v| v.to_str().ok()),
            Some(marker.as_str())
        );
        let status = resp.status();
        let v: serde_json::Value = resp.json().await.unwrap();
        if !status.is_success() {
            assert_eq!(v["error"]["type"], "invalid_request_error", "{status}");
        }
    }
    assert_eq!(
        models["profiles_blake3"],
        acn_mockllm::profile::embedded().unwrap().blake3
    );
    server.abort();
}
