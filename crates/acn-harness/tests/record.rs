//! HAR-30..34: what the harness records — every span TRC-10..14 requires with its
//! required attributes, usage through the frozen mapping, the counting method,
//! timing on the run's clock, and no content anywhere.

#![allow(clippy::unwrap_used, clippy::expect_used, clippy::panic)] // CON-19: tests are exempt

mod common;

use acn_harness::agent::Opts;
use acn_trace::model::AttrValue;
use common::{Spec, float, int, run_fixture, session, spans, text};
use serde_json::Value;

/// Cites: HAR-30, TRC-10, TRC-11, TRC-12, TRC-13, TRC-14, TRC-1
#[test]
fn every_span_carries_what_its_trc_clause_requires() {
    // Both tasks, with fan-out, in one bundle.
    let f = run_fixture(&common::smoke(), "auto");
    let w = acn_harness::run::run(&f.cfg).unwrap();
    let trace = common::read(&w.dir);
    let inv = acn_trace::schema::inventory().unwrap();
    for name in [
        "acn.session",
        "acn.turn",
        "chat",
        "execute_tool",
        "invoke_agent",
    ] {
        let these = spans(&trace, name);
        assert!(!these.is_empty(), "{name}");
        for a in inv.attributes().iter().filter(|a| {
            a.required
                && a.on.iter().any(|o| o == name)
                && a.producers.iter().any(|p| p == "acn-harness")
        }) {
            for s in &these {
                assert!(s.attrs.contains_key(&a.name), "{name} lacks {}", a.name);
            }
        }
        let spec = inv.spans().iter().find(|s| s.name == name).unwrap();
        for s in &these {
            let parent = trace
                .spans
                .iter()
                .find(|p| Some(p.span_id) == s.parent_span_id)
                .map_or("root", |p| p.name.as_str());
            assert!(
                spec.parents.iter().any(|p| p == parent),
                "{name} under {parent}"
            );
        }
    }
    let session = spans(&trace, "acn.session")[0];
    assert_eq!(text(session, "acn.role"), Some("treatment"));
    assert_eq!(text(session, "acn.backend"), Some("mockllm"));
    assert_eq!(
        text(session, "acn.run_id"),
        Some(w.run_id.to_hex().as_str())
    );
    for t in spans(&trace, "execute_tool") {
        assert_eq!(text(t, "acn.tool.placement"), Some("local"));
    }
    assert_eq!(spans(&trace, "acn.session").len(), 2, "one per task");
}

/// Cites: HAR-31, TRC-21
#[test]
fn usage_is_normalised_by_the_frozen_mapping_and_raw_fields_are_kept() {
    let r = session(Spec {
        model: "explicit",
        ..Spec::default()
    });
    for c in spans(&r.trace, "chat") {
        let input = int(c, "acn.call.input_tokens").unwrap();
        assert_eq!(
            int(c, "gen_ai.usage.input_tokens"),
            Some(input),
            "prompt_tokens"
        );
        assert_eq!(
            int(c, "acn.call.output_tokens"),
            int(c, "gen_ai.usage.output_tokens")
        );
        assert_eq!(
            int(c, "acn.cache.read_tokens"),
            int(c, "gen_ai.usage.cache_read.input_tokens")
        );
        assert_eq!(
            int(c, "acn.cache.write_tokens"),
            int(c, "gen_ai.usage.cache_creation.input_tokens")
        );
        let stop = text(c, "acn.call.stop_reason").unwrap();
        let raw = text(c, "acn.call.stop_reason_raw").unwrap();
        assert_eq!(
            (stop, raw),
            match raw {
                "tool_calls" => ("tool_use", "tool_calls"),
                _ => ("end_turn", "stop"),
            }
        );
    }
    assert!(
        spans(&r.trace, "chat")
            .iter()
            .any(|c| int(c, "acn.cache.write_tokens") > Some(0)),
        "explicit breakpoints write"
    );
}

/// Cites: HAR-32, TRC-12, MLM-11
#[test]
fn the_mock_counts_new_input_tokens_on_its_own_token_sequences() {
    let r = session(Spec::default());
    let chats = spans(&r.trace, "chat");
    assert!(
        chats
            .iter()
            .all(|c| text(c, "acn.call.new_input_tokens_method") == Some("tokens"))
    );
    assert_eq!(
        int(chats[0], "acn.call.new_input_tokens"),
        int(chats[0], "acn.call.input_tokens"),
        "the first call has no previous context"
    );
    // Call 1 against call 0's request followed by its response, recomputed from
    // the bodies with the mock's own prompt bytes.
    let profile = common::three().get("auto").unwrap().clone();
    let (b0, b1) = (&r.bodies[0].1, &r.bodies[1].1);
    let mut prev = b0.clone();
    let n0 = b0["messages"].as_array().unwrap().len();
    prev["messages"]
        .as_array_mut()
        .unwrap()
        .push(b1["messages"][n0].clone());
    let bytes = |b: &Value| acn_mockllm::prompt::prompt(b, &profile).unwrap().bytes;
    let (p, t) = (bytes(&prev), bytes(b1));
    let lcp = p.iter().zip(&t).take_while(|(a, b)| a == b).count() as i64;
    let input = int(chats[1], "acn.call.input_tokens").unwrap();
    assert_eq!(
        int(chats[1], "acn.call.new_input_tokens"),
        Some(input - lcp / 4)
    );
    assert!(
        chats
            .iter()
            .all(|c| int(c, "acn.call.new_input_tokens") <= int(c, "acn.call.input_tokens"))
    );
}

/// Cites: HAR-33, TRC-12
#[test]
fn timing_comes_from_the_run_clock_and_matches_the_mock_model() {
    // Profile: prefill 1 ms + 1 µs per new token + 0.1 µs per cached token, 1 ms
    // per output token, no jitter; stalls above 0.5 ms.
    let r = session(Spec {
        opts: Opts {
            stall_threshold_ms: 0.5,
            ..Opts::default()
        },
        ..Spec::default()
    });
    for (c, (_, body)) in spans(&r.trace, "chat").iter().zip(&r.bodies) {
        let input = int(c, "acn.call.input_tokens").unwrap();
        let cached = int(c, "acn.cache.read_tokens").unwrap();
        let ttft_ns = 1_000_000 + (input - cached) * 1_000 + cached * 100;
        assert!((float(c, "acn.call.ttft_ms").unwrap() - ttft_ns as f64 / 1e6).abs() < 1e-9);
        let out = int(c, "acn.call.output_tokens").unwrap();
        assert_eq!(c.end_ns - c.start_ns, ttft_ns + (out - 1) * 1_000_000);
        assert_eq!(
            int(c, "acn.call.wire_bytes_up"),
            Some(serde_json::to_vec(body).unwrap().len() as i64)
        );
        assert!(int(c, "acn.call.wire_bytes_down") > Some(0));
        assert_eq!(
            c.attrs.get("acn.call.streamed"),
            Some(&AttrValue::Bool(true))
        );
        let events: Vec<_> = r
            .trace
            .events
            .iter()
            .filter(|e| e.span_id == c.span_id)
            .collect();
        let first = events
            .iter()
            .find(|e| e.name == "acn.stream.first_token")
            .unwrap();
        assert_eq!(first.time_ns, c.start_ns + ttft_ns);
        let stalls = events
            .iter()
            .filter(|e| e.name == "acn.stream.stall")
            .count() as i64;
        if out >= 2 {
            assert_eq!(float(c, "acn.call.itl_p50_ms"), Some(1.0));
            assert_eq!(float(c, "acn.call.itl_p99_ms"), Some(1.0));
            assert_eq!(stalls, out - 1, "every 1 ms gap is above 0.5 ms");
        } else {
            assert!(
                float(c, "acn.call.itl_p50_ms").is_none(),
                "one token, no gap"
            );
        }
    }
    // A non-streamed call: the first token is the response.
    let plain = session(Spec {
        workload: common::smoke().replace("stream = true", "stream = false"),
        ..Spec::default()
    });
    let c = spans(&plain.trace, "chat")[1];
    assert_eq!(
        float(c, "acn.call.ttft_ms").unwrap(),
        (c.end_ns - c.start_ns) as f64 / 1e6
    );
    assert!(float(c, "acn.call.itl_p50_ms").is_none());
}

/// Cites: HAR-34, TRC-42
#[test]
fn no_message_content_reaches_a_span_or_an_event() {
    let r = session(Spec {
        task: 1,
        ..Spec::default()
    });
    let r0 = session(Spec::default());
    let needles = [
        "careful software engineer",
        "Open src/config.rs",
        "Investigate how configuration",
        "Find where the configuration file",
    ];
    for trace in [&r.trace, &r0.trace] {
        for attrs in trace
            .spans
            .iter()
            .map(|s| &s.attrs)
            .chain(trace.events.iter().map(|e| &e.attrs))
        {
            for v in attrs.values() {
                if let AttrValue::String(s) = v {
                    for n in needles {
                        assert!(!s.contains(n), "content in an attribute: {s}");
                    }
                    assert!(s.len() < 512, "nothing long enough to be content: {s}");
                }
            }
        }
    }
}

fn manifest(dir: &std::path::Path) -> Value {
    serde_json::from_slice(&std::fs::read(dir.join("manifest.json")).unwrap()).unwrap()
}

/// Cites: HAR-51, CON-29
#[test]
fn every_varied_knob_and_the_workload_enter_the_run_identity() {
    let base = run_fixture(&common::smoke(), "auto");
    let wa = acn_harness::run::run(&base.cfg).unwrap();
    let ma = manifest(&wa.dir);
    let smoke_hash = acn_harness::workload::Workload::parse(common::smoke().as_bytes())
        .unwrap()
        .hash
        .to_hex();
    assert_eq!(ma["workload_hash"], smoke_hash);
    assert_eq!(ma["params"]["hyp_status"], "candidate");
    assert_eq!(ma["params"]["arms"], "treatment");
    assert_eq!(ma["params"]["backend"], "mockllm");
    assert_eq!(ma["params"]["model"], "auto");
    // One knob more: its value is a parameter, and the identity moves.
    let mut knob = run_fixture(&common::smoke(), "auto");
    knob.cfg
        .vary
        .insert("tool_order_stable".into(), "false".into());
    let wb = acn_harness::run::run(&knob.cfg).unwrap();
    assert_eq!(
        manifest(&wb.dir)["params"]["vary.tool_order_stable"],
        "false"
    );
    assert_ne!(wa.run_id, wb.run_id);
    // One byte more in the workload: another hash, another identity.
    let edited = run_fixture(&format!("{}# edited\n", common::smoke()), "auto");
    let wc = acn_harness::run::run(&edited.cfg).unwrap();
    assert_ne!(manifest(&wc.dir)["workload_hash"], smoke_hash);
    assert_ne!(wa.run_id, wc.run_id);
    // The arm too.
    let mut control = run_fixture(&common::smoke(), "auto");
    control.cfg.arm = "control".into();
    assert_ne!(
        acn_harness::run::run(&control.cfg).unwrap().run_id,
        wa.run_id
    );
}

const CANDIDATE: &str = r#"[poc]
id = "zz"
title = "a test hypothesis"

[hypothesis]
statement = "s"

[varies]
tool_order_stable = { kind = "bool" }
provider = { kind = "enum", values = ["anthropic", "openai"] }
ratio = { kind = "range", min = 0, max = 10 }
"#;

/// Cites: HAR-50, HAR-10, HYP-9, HYP-6
#[test]
fn a_hypothesis_file_names_the_run_types_its_parameters_and_seeds_it() {
    use acn_harness::run::HypothesisArg;
    let run_with = |text: &str, vary: &[(&str, &str)]| {
        let mut f = run_fixture(&common::smoke(), "auto");
        let path = f.dir.path().join("zz.toml");
        std::fs::write(&path, text).unwrap();
        f.cfg.hypothesis = HypothesisArg::File(path);
        for (k, v) in vary {
            f.cfg.vary.insert((*k).into(), (*v).into());
        }
        (acn_harness::run::run(&f.cfg), f)
    };
    let with_seed = format!("{CANDIDATE}\n[design]\nseed = 1234\n");
    let (w, _f) = run_with(
        &with_seed,
        &[
            ("tool_order_stable", "false"),
            ("provider", "openai"),
            ("ratio", "5"),
        ],
    );
    let w = w.unwrap();
    let m = manifest(&w.dir);
    assert_eq!(m["hypothesis"]["id"], "zz");
    assert_eq!(m["hypothesis"]["status"], "candidate");
    assert_eq!(
        m["hypothesis"]["hash"],
        acn_trace::identity::Digest::of(with_seed.as_bytes()).to_hex()
    );
    assert_eq!(m["seed"], "1234", "a candidate's [design].seed");
    assert_eq!(
        m["params"]["vary.ratio"], "5.0",
        "a range is a float (CON-27(c))"
    );
    assert_eq!(m["params"]["vary.provider"], "openai");
    let session = common::read(&w.dir)
        .spans
        .into_iter()
        .find(|s| s.name == "acn.session")
        .unwrap();
    assert_eq!(text(&session, "acn.hypothesis.id"), Some("zz"));
    // Without a seed the seed is derived from the file (HYP-9).
    let (w, _f) = run_with(CANDIDATE, &[]);
    let hash = acn_trace::identity::Digest::of(CANDIDATE.as_bytes());
    let derived = acn_trace::identity::derived_seed(
        &acn_trace::identity::Preimage::new("acn-bench/hypothesis_seed/v1")
            .unwrap()
            .digest(&hash)
            .finish(),
    );
    assert_eq!(manifest(&w.unwrap().dir)["seed"], derived.to_string());
    // Only [varies] parameters, and only values in their domains.
    for (vary, needle) in [
        (
            ("backfill_mode", "tail_restate"),
            "not a [varies] parameter",
        ),
        (("provider", "opneai"), "outside its domain"),
        (("ratio", "11"), "outside its domain"),
    ] {
        let (r, f) = run_with(CANDIDATE, &[vary]);
        let err = r.unwrap_err().to_string();
        assert!(err.contains(needle), "{vary:?}: {err}");
        assert!(
            !f.cfg.runs_dir.exists(),
            "refused before anything is written"
        );
    }
    // A misspelt kind is refused, never read as a string.
    let (r, _f) = run_with(
        &CANDIDATE.replace("kind = \"range\"", "kind = \"ranged\""),
        &[],
    );
    assert!(r.unwrap_err().to_string().contains("kind `ranged`"));
}

/// Cites: HAR-50, HYP-9, HYP-3
#[test]
fn a_frozen_file_may_not_choose_its_seed() {
    use acn_harness::run::HypothesisArg;
    let mut f = run_fixture(&common::smoke(), "auto");
    let root = f.dir.path().join("kit");
    std::fs::create_dir_all(root.join("hypotheses")).unwrap();
    std::fs::write(root.join("env-hash.json"), "{}").unwrap();
    let path = root.join("hypotheses/zz.toml");
    std::fs::write(&path, format!("{CANDIDATE}\n[design]\nseed = 1234\n")).unwrap();
    f.cfg.start_dir = root;
    f.cfg.hypothesis = HypothesisArg::File(path);
    let err = acn_harness::run::run(&f.cfg).unwrap_err().to_string();
    assert!(
        err.contains("frozen file carries no `[design].seed`"),
        "{err}"
    );
}

/// Cites: HAR-30, TRC-12
#[test]
fn every_call_with_usage_records_the_regime_its_counts_derive() {
    let f = run_fixture(&common::smoke(), "auto");
    let w = acn_harness::run::run(&f.cfg).unwrap();
    let trace = common::read(&w.dir);
    let chats = spans(&trace, "chat");
    assert!(!chats.is_empty());
    let mut seen = std::collections::BTreeSet::new();
    for c in chats {
        let want = acn_trace::ingest::derive_regime(
            int(c, "acn.call.input_tokens"),
            int(c, "acn.cache.read_tokens"),
        );
        assert_eq!(text(c, "acn.call.regime"), want, "{:?}", c.attrs);
        if let Some(r) = want {
            seen.insert(r);
        }
    }
    // The smoke workload repeats its prefix on the mock: some calls prefill,
    // and some read from the cache.
    assert!(
        seen.contains("prefill") && seen.contains("midfill"),
        "{seen:?}"
    );
    // The bundle's views carry it, so it verifies with its views (TRC-35).
    acn_trace::bundle::verify_views(&w.dir).unwrap();
}
