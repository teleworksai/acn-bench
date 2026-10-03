//! The `live` tier (SPEC 040 §9): one session of the smoke workload against each
//! real provider, with cached tokens recorded and normalised. Needs the
//! `real-api` feature, credentials in the environment and network access; run by
//! hand: `cargo test -p acn-harness --features real-api --test live_providers -- --ignored`.
//! The model is read from `ACN_LIVE_<PROVIDER>_MODEL`.

#![allow(clippy::unwrap_used, clippy::expect_used, clippy::panic)] // CON-19: tests are exempt
#![cfg(feature = "real-api")]

mod common;

use acn_harness::wire::Backend;
use acn_trace::identity::Mode;
use common::{int, run_fixture, spans, text};

fn live(backend: Backend, model_var: &str) {
    let model = std::env::var(model_var).unwrap_or_else(|_| panic!("set {model_var}"));
    let mut f = run_fixture(&common::smoke(), &model);
    f.cfg.backend = backend;
    f.cfg.mode = Mode::Live;
    let w = acn_harness::run::run(&f.cfg).unwrap();
    let trace = common::read(&w.dir);
    let chats = spans(&trace, "chat");
    assert!(!chats.is_empty());
    for c in &chats {
        assert!(int(c, "acn.call.input_tokens").is_some());
        assert_eq!(
            text(c, "acn.call.new_input_tokens_method"),
            Some("bytes_scaled")
        );
    }
    // The smoke workload stays below the providers' cache minimum (1024 tokens
    // or more), so a read is not expected; the mapping of the counts is.
    for c in &chats {
        assert!(int(c, "acn.cache.read_tokens").is_some() || backend == Backend::Openai);
        assert!(int(c, "acn.cache.write_tokens").is_some());
    }
}

/// Cites: HAR-20, HAR-31
#[test]
#[ignore = "live tier: real-api, credentials and network"]
fn anthropic_records_cache_reads_and_writes() {
    live(Backend::Anthropic, "ACN_LIVE_ANTHROPIC_MODEL");
}

/// Cites: HAR-20, HAR-31
#[test]
#[ignore = "live tier: real-api, credentials and network"]
fn openai_records_cached_prompt_tokens() {
    live(Backend::Openai, "ACN_LIVE_OPENAI_MODEL");
}
