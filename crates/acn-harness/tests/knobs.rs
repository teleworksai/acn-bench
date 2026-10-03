//! HAR-10..17: each knob's effect on the request bytes, the knob map, and the
//! direction each knob moves cached tokens on each of the mock's three cache
//! models (MLM-21..23), with test profiles small enough to cache the smoke
//! workload.

#![allow(clippy::unwrap_used, clippy::expect_used, clippy::panic)] // CON-19: tests are exempt

mod common;

use std::collections::BTreeMap;

use acn_harness::context::{Context, Dialect, Msg, Sampling, ToolDef};
use acn_harness::knobs::{Knobs, Placement};
use acn_harness::run::typed_vary;
use acn_trace::identity::Value as P;
use common::{MARKER, Spec, cached, int, session, spans, text};
use serde_json::{Value, json};

fn system_text(body: &Value) -> String {
    body["messages"][0]["content"][0]["text"]
        .as_str()
        .unwrap()
        .to_owned()
}

/// Cites: HAR-10
#[test]
fn the_knob_map_is_six_knobs_with_the_shipped_defaults_recorded_whole() {
    assert_eq!(
        Knobs::default().to_json(),
        r#"{"backfill_mode":"mid_prefix","cache_breakpoint_placement":"system_only","compaction_trigger":"window_full","fanout_prompting":"per_child","timestamp_in_system_prompt":true,"tool_order_stable":true}"#,
        "the control of hypotheses/p4.toml, keys sorted"
    );
    let raw = |k: &str, v: &str| BTreeMap::from([(k.to_owned(), v.to_owned())]);
    let k =
        Knobs::from_vary(&typed_vary(&raw("tool_order_stable", "false"), None).unwrap()).unwrap();
    assert!(!k.tool_order_stable);
    assert!(k.to_json().contains(r#""tool_order_stable":false"#));
    // A value outside a domain, and a name that is not a knob, are refused.
    let bad = BTreeMap::from([("backfill_mode".to_owned(), P::Str("sideways".into()))]);
    assert!(Knobs::from_vary(&bad).is_err());
    assert!(typed_vary(&raw("timestamp_in_system_prompt", "maybe"), None).is_err());
    let err = typed_vary(&raw("tool_order_stabel", "true"), None).unwrap_err();
    assert!(err.to_string().contains("not a knob"), "{err}");
    // With a hypothesis, its [varies] parameters are accepted and typed by kind.
    let varies = BTreeMap::from([
        ("provider".to_owned(), "enum".to_owned()),
        ("ratio".to_owned(), "range".to_owned()),
    ]);
    let typed = typed_vary(
        &BTreeMap::from([
            ("provider".to_owned(), "anthropic".to_owned()),
            ("ratio".to_owned(), "5".to_owned()),
        ]),
        Some(&varies),
    )
    .unwrap();
    assert_eq!(
        typed["ratio"],
        P::Float(5.0),
        "a range is a float (CON-27(c))"
    );
    assert!(typed_vary(&raw("tool_order_stable", "true"), Some(&varies)).is_err());
}

/// Cites: HAR-11
#[test]
fn the_timestamp_is_the_turns_start_in_ms_on_the_line_after_the_marker() {
    let on = session(Spec::default());
    let turns = spans(&on.trace, "acn.turn");
    for (t, body) in &on.bodies {
        let sys = system_text(body);
        let mut lines = sys.lines();
        assert_eq!(lines.next(), Some(format!("Session: {MARKER}").as_str()));
        let start = turns
            .iter()
            .rev()
            .find(|tr| tr.start_ns <= *t)
            .unwrap()
            .start_ns;
        assert_eq!(
            lines.next(),
            Some(format!("Current time: {}", start / 1_000_000).as_str())
        );
    }
    let stamps: std::collections::BTreeSet<String> =
        on.bodies.iter().map(|(_, b)| system_text(b)).collect();
    assert_eq!(stamps.len(), 3, "one stamp per turn");
    let off = session(Spec {
        vary: &[("timestamp_in_system_prompt", "false")],
        ..Spec::default()
    });
    assert!(
        off.bodies
            .iter()
            .all(|(_, b)| !system_text(b).contains("Current time"))
    );
}

fn tool_names(body: &Value) -> Vec<String> {
    body["tools"]
        .as_array()
        .unwrap()
        .iter()
        .map(|t| t["function"]["name"].as_str().unwrap().to_owned())
        .collect()
}

/// Cites: HAR-12
#[test]
fn tools_come_in_workload_order_or_freshly_permuted_per_request() {
    let stable = session(Spec::default());
    assert!(
        stable
            .bodies
            .iter()
            .all(|(_, b)| tool_names(b) == ["read_file", "grep"])
    );
    let shuffled = session(Spec {
        vary: &[("tool_order_stable", "false")],
        ..Spec::default()
    });
    let orders: std::collections::BTreeSet<Vec<String>> =
        shuffled.bodies.iter().map(|(_, b)| tool_names(b)).collect();
    assert_eq!(orders.len(), 2, "both orders of two tools occur");
    let again = session(Spec {
        vary: &[("tool_order_stable", "false")],
        ..Spec::default()
    });
    assert_eq!(
        shuffled.bodies, again.bodies,
        "the permutations come from the seed (harness.knobs)"
    );
}

fn tool_message(body: &Value) -> Option<String> {
    body["messages"]
        .as_array()
        .unwrap()
        .iter()
        .find(|m| m["role"] == "tool")
        .map(|m| m["content"].as_str().unwrap().to_owned())
}

fn first_of_turn(r: &common::Ran, k: usize) -> &Value {
    let start = spans(&r.trace, "acn.turn")[k].start_ns;
    &r.bodies.iter().find(|(t, _)| *t == start).unwrap().1
}

/// Cites: HAR-13
#[test]
fn backfill_replaces_in_place_or_restates_at_the_tail() {
    // Turn 1 of `edit` lists `updates = [0]`: the result of the first read changed.
    let mid = session(Spec {
        vary: &[("backfill_mode", "mid_prefix")],
        ..Spec::default()
    });
    let tail = session(Spec {
        vary: &[("backfill_mode", "tail_restate")],
        ..Spec::default()
    });
    let before = tool_message(first_of_turn(&mid, 0)).is_none();
    assert!(before, "turn 0's first request has no result yet");
    let original = tool_message(&mid.bodies[1].1).unwrap();
    assert_eq!(tool_message(&tail.bodies[1].1).unwrap(), original);
    // mid_prefix: the earlier message now holds the new content.
    let m = first_of_turn(&mid, 1);
    assert_ne!(tool_message(m).unwrap(), original);
    let last_user = |b: &Value| {
        b["messages"]
            .as_array()
            .unwrap()
            .iter()
            .rev()
            .find(|m| m["role"] == "user")
            .unwrap()["content"]
            .as_str()
            .unwrap()
            .to_owned()
    };
    assert!(last_user(m).starts_with("I changed src/config.rs"));
    // tail_restate: the earlier message is untouched; the turn restates it.
    let t = first_of_turn(&tail, 1);
    assert_eq!(tool_message(t).unwrap(), original);
    let restated = last_user(t);
    assert!(restated.starts_with("Updated result 0:\n"), "{restated}");
    assert_eq!(
        restated.lines().nth(1).unwrap(),
        tool_message(m).unwrap(),
        "the same new content, drawn the same way"
    );
}

/// Cites: HAR-14
#[test]
fn fork_children_start_from_the_spawning_context_per_child_from_their_own() {
    let fork = session(Spec {
        task: 1,
        vary: &[("fanout_prompting", "fork_from_prefix")],
        ..Spec::default()
    });
    let per = session(Spec {
        task: 1,
        ..Spec::default()
    });
    // Bodies: the spawning call, then the two children's first calls together.
    let spawning = &fork.bodies[0].1;
    for (_, child) in &fork.bodies[1..3] {
        let msgs = child["messages"].as_array().unwrap();
        let parent = spawning["messages"].as_array().unwrap();
        assert_eq!(
            &msgs[..parent.len()],
            parent.as_slice(),
            "the parent's context"
        );
        assert_eq!(msgs.len(), parent.len() + 1);
        assert!(
            msgs.last().unwrap()["content"]
                .as_str()
                .unwrap()
                .contains("(child ")
        );
        assert_eq!(child["tools"], spawning["tools"]);
    }
    for (_, child) in &per.bodies[1..3] {
        assert!(system_text(child).ends_with(
            "You are a helper. Investigate what you are asked and answer in two sentences."
        ));
        assert_eq!(tool_names(child), ["read_file"]);
        assert_eq!(
            child["messages"].as_array().unwrap().len(),
            2,
            "system and instruction"
        );
    }
    let shared = |r: &common::Ran| {
        spans(&r.trace, "invoke_agent")
            .iter()
            .map(|a| int(a, "acn.fanout.shared_prefix_tokens").unwrap())
            .collect::<Vec<_>>()
    };
    let (f, p) = (shared(&fork), shared(&per));
    assert!(f.iter().zip(&p).all(|(f, p)| f > p), "{f:?} vs {p:?}");
}

/// Cites: HAR-15
#[test]
fn each_compaction_trigger_fires_on_its_own_condition() {
    let compactions = |r: &common::Ran| {
        spans(&r.trace, "acn.turn")
            .iter()
            .map(|t| text(t, "acn.turn.compaction").unwrap().to_owned())
            .collect::<Vec<_>>()
    };
    // window_full: the estimate ⌈canonical bytes / 4⌉ reaches 1000 in turn 2 only.
    let full = session(Spec::default());
    assert_eq!(compactions(&full), ["none", "none", "window_full"]);
    // read_cost_threshold: the previous call's uncached input exceeded 400.
    let cost = session(Spec {
        vary: &[("compaction_trigger", "read_cost_threshold")],
        ..Spec::default()
    });
    assert_eq!(
        compactions(&cost),
        ["none", "read_cost_threshold", "read_cost_threshold"]
    );
    // It depends on what the provider cached: nothing cached, it fires sooner.
    let cold = session(Spec {
        vary: &[("compaction_trigger", "read_cost_threshold")],
        profiles: common::profiles(&[common::profile(
            "auto",
            "automatic_prefix",
            &["min_cacheable_tokens = 100000"],
        )]),
        ..Spec::default()
    });
    // Where in turn 1 the compaction call comes: warm, after the turn's first
    // call; cold, before it, since turn 0's last call left 483 tokens uncached.
    let position_in_turn_1 = |r: &common::Ran| {
        let t1 = spans(&r.trace, "acn.turn")[1];
        r.bodies
            .iter()
            .filter(|(t, _)| *t >= t1.start_ns && *t <= t1.end_ns)
            .position(|(_, b)| {
                b["messages"].as_array().unwrap().last().unwrap()["content"]
                    .as_str()
                    .is_some_and(|c| c.starts_with("Summarise the conversation"))
            })
            .unwrap()
    };
    assert_eq!(position_in_turn_1(&cost), 1);
    assert_eq!(position_in_turn_1(&cold), 0);
}

fn count_marks(v: &Value) -> usize {
    match v {
        Value::Object(m) => {
            usize::from(m.contains_key("cache_control"))
                + m.values().map(count_marks).sum::<usize>()
        }
        Value::Array(a) => a.iter().map(count_marks).sum(),
        _ => 0,
    }
}

/// Cites: HAR-16
#[test]
fn breakpoints_go_where_the_placement_says_and_only_where_they_are_read() {
    let body = |placement: &str| {
        session(Spec {
            model: "explicit",
            vary: &[("cache_breakpoint_placement", placement)],
            ..Spec::default()
        })
        .bodies[1]
            .1
            .clone()
    };
    let none = body("none");
    assert_eq!(count_marks(&none), 0);
    let sys = body("system_only");
    assert_eq!(count_marks(&sys), 1);
    assert!(
        sys["messages"][0]["content"][0]
            .get("cache_control")
            .is_some()
    );
    let tools = body("system_and_tools");
    assert_eq!(count_marks(&tools), 2);
    assert!(
        tools["tools"][1].get("cache_control").is_some(),
        "the last tool"
    );
    let tail = body("rolling_tail");
    assert_eq!(count_marks(&tail), 2);
    let last = tail["messages"].as_array().unwrap().last().unwrap();
    assert!(last.get("cache_control").is_some(), "the last message");
    // Breakpoints are sent only on the Messages dialect and to the mock.
    let ctx = Context {
        system: "s".into(),
        tools: vec![ToolDef {
            name: "t".into(),
            description: "d".into(),
            parameters: json!({}),
        }],
        messages: vec![Msg::User { text: "u".into() }],
    };
    let sampling = Sampling {
        max_tokens: 1,
        temperature: 0.0,
        stream: false,
    };
    for d in [Dialect::ChatCompletions, Dialect::Messages] {
        for p in [
            Placement::SystemOnly,
            Placement::SystemAndTools,
            Placement::RollingTail,
        ] {
            assert_eq!(count_marks(&ctx.encode(d, "m", sampling, p, false)), 0);
            assert!(count_marks(&ctx.encode(d, "m", sampling, p, true)) > 0);
        }
    }
}

/// The cached tokens of both smoke tasks under one knob setting.
fn total(model: &str, knob: &str, value: &str) -> i64 {
    (0..2)
        .map(|task| {
            let r = session(Spec {
                task,
                model,
                vary: &[(knob, value)],
                ..Spec::default()
            });
            r.result.unwrap();
            cached(&r.trace).iter().sum::<i64>()
        })
        .sum()
}

/// Cites: HAR-11, HAR-12, HAR-13, HAR-14, HAR-16, MLM-21, MLM-22, MLM-23
#[test]
fn each_knob_moves_cached_tokens_the_way_its_habit_should_on_each_cache_model() {
    // (knob, cache-friendly value, cache-hostile value): friendly reads more.
    let knobs = [
        ("timestamp_in_system_prompt", "false", "true"),
        ("tool_order_stable", "true", "false"),
        ("backfill_mode", "tail_restate", "mid_prefix"),
        ("fanout_prompting", "fork_from_prefix", "per_child"),
    ];
    for model in ["explicit", "auto", "blocks"] {
        for (knob, friendly, hostile) in knobs {
            let (f, h) = (total(model, knob, friendly), total(model, knob, hostile));
            assert!(
                f > h,
                "{model}: {knob}={friendly} read {f}, {hostile} read {h}"
            );
        }
    }
    // Placement matters only to explicit breakpoints (MLM-21): nothing marked,
    // nothing read; marking the tools as well reads more.
    let p = |model, v| total(model, "cache_breakpoint_placement", v);
    assert_eq!(p("explicit", "none"), 0);
    assert!(p("explicit", "system_only") > 0);
    assert!(p("explicit", "system_and_tools") > p("explicit", "system_only"));
    // ADR-17: MLM-21 reads only prefixes the request marks, so one moving tail
    // breakpoint is never read again and reads what `system_only` reads.
    assert_eq!(p("explicit", "rolling_tail"), p("explicit", "system_only"));
    for model in ["auto", "blocks"] {
        let all: Vec<i64> = ["none", "system_only", "system_and_tools", "rolling_tail"]
            .iter()
            .map(|v| p(model, v))
            .collect();
        assert!(all.windows(2).all(|w| w[0] == w[1]), "{model}: {all:?}");
    }
}

/// Cites: HAR-17
#[test]
fn a_knob_changes_no_byte_where_its_clause_does_not_apply() {
    let bodies = |task: usize, vary: &'static [(&'static str, &'static str)], huge: bool| {
        let mut workload = common::smoke();
        if huge {
            workload = workload
                .replace("compact_at_tokens = 1000", "compact_at_tokens = 1000000")
                .replace(
                    "read_cost_threshold_tokens = 400",
                    "read_cost_threshold_tokens = 1000000",
                );
        }
        session(Spec {
            task,
            workload,
            vary,
            ..Spec::default()
        })
        .bodies
    };
    // A compaction trigger that never fires.
    assert_eq!(
        bodies(0, &[("compaction_trigger", "window_full")], true),
        bodies(0, &[("compaction_trigger", "read_cost_threshold")], true)
    );
    // Fan-out prompting on a task that never fans out.
    assert_eq!(
        bodies(0, &[("fanout_prompting", "fork_from_prefix")], false),
        bodies(0, &[("fanout_prompting", "per_child")], false)
    );
    // Backfill on a task with no updates.
    assert_eq!(
        bodies(1, &[("backfill_mode", "tail_restate")], false),
        bodies(1, &[("backfill_mode", "mid_prefix")], false)
    );
}
