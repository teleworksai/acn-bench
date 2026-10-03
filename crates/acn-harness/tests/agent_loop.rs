//! HAR-1..5: the agent loop — turns, simulated tools, the checker, compaction and
//! fan-out — on the smoke workload against the in-process mock.

#![allow(clippy::unwrap_used, clippy::expect_used, clippy::panic)] // CON-19: tests are exempt

mod common;

use common::{Spec, children, int, profile, profiles, session, smoke, spans, text};

/// Cites: HAR-1, HAR-2, HAR-3
#[test]
fn a_turn_calls_tools_until_a_response_asks_for_none() {
    let r = session(Spec::default());
    r.result.unwrap();
    let turns = spans(&r.trace, "acn.turn");
    assert_eq!(turns.len(), 3, "task `edit` has three turns");
    for (k, turn) in turns.iter().enumerate() {
        assert_eq!(text(turn, "acn.turn.outcome"), Some("success"), "turn {k}");
        let kids = children(&r.trace, turn);
        let chats: Vec<_> = kids.iter().filter(|s| s.name == "chat").collect();
        let tools: Vec<_> = kids.iter().filter(|s| s.name == "execute_tool").collect();
        assert_eq!(tools.len(), 1, "the mock asks for one tool per turn");
        assert_eq!(
            text(chats[0], "acn.call.stop_reason"),
            Some("tool_use"),
            "turn {k}"
        );
        assert_eq!(
            text(chats.last().unwrap(), "acn.call.stop_reason"),
            Some("end_turn")
        );
        // HAR-2: a simulated read_file, from the workload's ranges.
        let t = tools[0];
        assert_eq!(text(t, "gen_ai.tool.name"), Some("read_file"));
        assert_eq!(text(t, "acn.tool.class"), Some("file"));
        let bytes = int(t, "acn.tool.result_bytes").unwrap();
        assert!((600..=1200).contains(&bytes), "{bytes}");
        let dur = t.end_ns - t.start_ns;
        assert!((1_000_000..=3_000_000).contains(&dur), "{dur}");
        assert_eq!(t.start_ns, chats[0].end_ns, "the tool runs on the response");
        assert_eq!(
            chats[1].start_ns, t.end_ns,
            "the next call follows the result"
        );
    }
    // Think time between turns, from the workload's range (HAR-1).
    let gap = turns[1].start_ns - turns[0].end_ns;
    assert!((1_000_000_000..=3_000_000_000).contains(&gap), "{gap}");
    // The tool result went back to the model, byte for byte.
    let req = &r.bodies[1].1["messages"];
    let result = req.as_array().unwrap().last().unwrap();
    assert_eq!(result["role"], "tool");
    assert_eq!(
        result["content"].as_str().unwrap().len() as i64,
        int(spans(&r.trace, "execute_tool")[0], "acn.tool.result_bytes").unwrap()
    );
}

fn outcome_of(workload: String, model_overrides: &[&str]) -> Vec<String> {
    let r = session(Spec {
        workload,
        profiles: profiles(&[profile("auto", "automatic_prefix", model_overrides)]),
        ..Spec::default()
    });
    r.result.unwrap();
    spans(&r.trace, "acn.turn")
        .iter()
        .map(|t| text(t, "acn.turn.outcome").unwrap().to_owned())
        .collect()
}

/// Cites: HAR-1, HAR-3
#[test]
fn a_turn_ends_aborted_at_the_cap_timeout_at_its_deadline_and_failure_by_the_checker() {
    // The call cap: one call, which asks for a tool, and no second.
    let capped = smoke().replace("max_calls_per_turn = 6", "max_calls_per_turn = 1");
    assert_eq!(outcome_of(capped, &[]), ["aborted", "aborted", "aborted"]);
    // A model that never calls a tool fails turn 0's checker (expect read_file);
    // turns 1 and 2 expect nothing and succeed.
    assert_eq!(
        outcome_of(smoke(), &["tool_calls_per_turn = 0"]),
        ["failure", "success", "success"]
    );
    // A 1 ms deadline on turn 1 passes during its first call.
    let tight = smoke().replace("deadline_ms = 600_000", "deadline_ms = 1");
    assert_eq!(outcome_of(tight, &[]), ["success", "timeout", "success"]);
}

/// Cites: HAR-4
#[test]
fn compaction_replaces_the_history_with_a_summary_and_is_recorded() {
    let r = session(Spec::default());
    let turns = spans(&r.trace, "acn.turn");
    let compactions: Vec<_> = turns
        .iter()
        .map(|t| text(t, "acn.turn.compaction").unwrap())
        .collect();
    assert_eq!(compactions, ["none", "none", "window_full"]);
    // The compaction call ends with the summary instruction; the call after it
    // starts from the summary and keeps only the current turn.
    let summary_instruction =
        "Summarise the conversation so far in a few sentences, keeping file names and decisions.";
    let (i, compaction) = r
        .bodies
        .iter()
        .enumerate()
        .find(|(_, (_, b))| {
            b["messages"].as_array().unwrap().last().unwrap()["content"] == summary_instruction
        })
        .unwrap();
    assert_eq!(
        compaction.1["max_completion_tokens"], 32,
        "summary_max_tokens"
    );
    let after = r.bodies[i + 1].1["messages"].as_array().unwrap();
    let first_user = after.iter().find(|m| m["role"] == "user").unwrap();
    assert!(
        first_user["content"]
            .as_str()
            .unwrap()
            .starts_with("Summary of earlier conversation:\n")
    );
    let all = serde_json::to_string(after).unwrap();
    assert!(!all.contains("Open src/config.rs"), "turn 0 is gone");
    assert!(all.contains("Now make the error message name the offending key."));
    // The compaction call is a chat of the lineage like any other.
    let third: Vec<_> = children(&r.trace, turns[2])
        .into_iter()
        .filter(|s| s.name == "chat")
        .collect();
    assert_eq!(third.len(), 3);
    assert_eq!(
        third
            .iter()
            .map(|c| int(c, "acn.call.index").unwrap())
            .collect::<Vec<_>>(),
        [0, 1, 2]
    );
}

/// Cites: HAR-5
#[test]
fn fan_out_spawns_width_children_that_run_together_and_answer_as_one_result() {
    let r = session(Spec {
        task: 1,
        ..Spec::default()
    });
    r.result.unwrap();
    let tool = spans(&r.trace, "execute_tool")
        .into_iter()
        .find(|s| text(s, "acn.tool.class") == Some("subagent"))
        .unwrap();
    assert_eq!(int(tool, "acn.tool.requesting_call"), Some(0));
    let agents: Vec<_> = children(&r.trace, tool)
        .into_iter()
        .filter(|s| s.name == "invoke_agent")
        .collect();
    assert_eq!(agents.len(), 2, "width = 2");
    let mut first_calls = Vec::new();
    for a in &agents {
        assert_eq!(int(a, "acn.fanout.depth"), Some(1));
        assert_eq!(int(a, "acn.fanout.width"), Some(2));
        assert_eq!(int(a, "acn.fanout.parent_turn"), Some(0));
        assert_eq!(int(a, "acn.fanout.parent_call"), Some(0));
        assert_eq!(a.start_ns, tool.start_ns);
        let calls: Vec<_> = children(&r.trace, a)
            .into_iter()
            .filter(|s| s.name == "chat")
            .collect();
        assert_eq!(
            int(calls[0], "acn.call.index"),
            Some(0),
            "each lineage counts from 0"
        );
        first_calls.push(calls[0].start_ns);
        assert!(
            children(&r.trace, a)
                .iter()
                .all(|s| s.name != "invoke_agent")
        );
    }
    assert_eq!(
        first_calls[0], first_calls[1],
        "the children start together"
    );
    assert_eq!(
        tool.end_ns,
        agents.iter().map(|a| a.end_ns).max().unwrap(),
        "the tool ends with its last child"
    );
    // Their answers, joined in child order, are the tool's result.
    let main_after = &r.bodies.last().unwrap().1["messages"];
    let result = main_after.as_array().unwrap().last().unwrap();
    assert_eq!(result["role"], "tool");
    let content = result["content"].as_str().unwrap();
    assert_eq!(
        content.len() as i64,
        int(tool, "acn.tool.result_bytes").unwrap()
    );
    assert_eq!(content.split('\n').count(), 2);
    // Under fork_from_prefix a child sees the parent's tools but cannot spawn.
    let fork = session(Spec {
        task: 1,
        vary: &[("fanout_prompting", "fork_from_prefix")],
        ..Spec::default()
    });
    let refused: Vec<_> = spans(&fork.trace, "execute_tool")
        .into_iter()
        .filter(|s| text(s, "gen_ai.tool.name") == Some("delegate"))
        .filter(|s| text(s, "acn.tool.class") == Some("other"))
        .collect();
    assert_eq!(refused.len(), 2, "each child's call to delegate is refused");
    assert_eq!(
        spans(&fork.trace, "invoke_agent").len(),
        2,
        "and nothing nests"
    );
}

/// Cites: HAR-1, HAR-13
#[test]
fn several_tools_in_one_response_run_in_order_with_one_result_each() {
    use acn_harness::wire::Backend;
    use common::{Scripted, chat_reply, ok, scripted};
    let env = Scripted::new(vec![
        ok(&chat_reply(None, &[("a", "read_file"), ("b", "grep")])),
        ok(&chat_reply(Some("done"), &[])),
        ok(&chat_reply(Some("done"), &[])),
        ok(&chat_reply(Some("done"), &[])),
    ]);
    let (trace, result) = scripted(&env, &smoke(), 0, Backend::Openai);
    result.unwrap();
    let ids: Vec<&str> = spans(&trace, "execute_tool")
        .iter()
        .map(|t| text(t, "gen_ai.tool.call.id").unwrap())
        .collect();
    assert_eq!(ids, ["a", "b"], "in the order the response gave them");
    let second: serde_json::Value = serde_json::from_slice(&env.bodies.borrow()[1]).unwrap();
    let results: Vec<&str> = second["messages"]
        .as_array()
        .unwrap()
        .iter()
        .filter(|m| m["role"] == "tool")
        .map(|m| m["tool_call_id"].as_str().unwrap())
        .collect();
    assert_eq!(results, ["a", "b"], "one result message per tool, in order");
    // Turn 1 updates ordinal 0: the first result, read_file's, not grep's.
    let third: serde_json::Value = serde_json::from_slice(&env.bodies.borrow()[2]).unwrap();
    let msgs = third["messages"].as_array().unwrap();
    let first_result = msgs.iter().find(|m| m["tool_call_id"] == "a").unwrap();
    let second_result = msgs.iter().find(|m| m["tool_call_id"] == "b").unwrap();
    let before: serde_json::Value = serde_json::from_slice(&env.bodies.borrow()[1]).unwrap();
    let was = |id: &str| {
        before["messages"]
            .as_array()
            .unwrap()
            .iter()
            .find(|m| m["tool_call_id"] == id)
            .unwrap()["content"]
            .clone()
    };
    assert_ne!(
        first_result["content"],
        was("a"),
        "replaced in place (mid_prefix)"
    );
    assert_eq!(second_result["content"], was("b"), "and nothing else");
}

/// Cites: HAR-1, HAR-21
#[test]
fn a_malformed_answer_is_an_error_of_the_call_not_a_finished_turn() {
    use acn_harness::wire::Backend;
    use common::{Scripted, chat_reply, ok, scripted};
    let no_message = ok(&serde_json::json!({ "choices": [], "usage": { "prompt_tokens": 1 } }));
    let nameless = ok(&chat_reply(None, &[("x", "")]));
    let not_json = acn_harness::wire::Exchange {
        status: 200,
        body: b"<html>gateway</html>".to_vec(),
        ..acn_harness::wire::Exchange::default()
    };
    let env = Scripted::new(vec![no_message, nameless, not_json]);
    let (trace, result) = scripted(&env, &smoke(), 0, Backend::Openai);
    result.unwrap();
    for (turn, chat) in spans(&trace, "acn.turn").iter().zip(spans(&trace, "chat")) {
        assert_eq!(text(turn, "acn.turn.outcome"), Some("aborted"));
        assert_eq!(
            text(chat, "acn.call.error_class"),
            Some("malformed_response")
        );
        assert_eq!(text(chat, "acn.call.stop_reason"), Some("other"));
        assert_eq!(int(chat, "acn.call.retries"), Some(0), "not retried");
    }
}

/// Cites: HAR-1, HAR-3
#[test]
fn an_answer_that_arrives_after_the_deadline_is_a_timeout() {
    // No tools: the first call answers, 1.2 ms in, past a 1 ms deadline.
    let r = session(Spec {
        workload: smoke().replace("deadline_ms = 600_000", "deadline_ms = 1"),
        profiles: profiles(&[profile(
            "auto",
            "automatic_prefix",
            &["tool_calls_per_turn = 0"],
        )]),
        ..Spec::default()
    });
    r.result.unwrap();
    let turn = spans(&r.trace, "acn.turn")[1];
    assert_eq!(text(turn, "acn.turn.outcome"), Some("timeout"));
    assert!(common::float(turn, "acn.turn.first_useful_result_ms").is_none());
    // A call still in flight at the deadline is abandoned there.
    let slow = session(Spec {
        workload: smoke().replace("deadline_ms = 600_000", "deadline_ms = 1"),
        profiles: profiles(&[profile(
            "auto",
            "automatic_prefix",
            &["tool_calls_per_turn = 0", "prefill_base_ns = 50000000"],
        )]),
        ..Spec::default()
    });
    let turn = spans(&slow.trace, "acn.turn")[1];
    let chat = children(&slow.trace, turn)
        .into_iter()
        .find(|s| s.name == "chat")
        .unwrap();
    assert_eq!(text(chat, "acn.call.error_class"), Some("deadline"));
    assert_eq!(chat.end_ns, turn.start_ns + 1_000_000);
    assert_eq!(text(turn, "acn.turn.outcome"), Some("timeout"));
}

/// Cites: HAR-4
#[test]
fn the_summary_a_compaction_call_returns_becomes_the_history() {
    let r = session(Spec {
        workload: smoke().replace("compact_at_tokens = 1000", "compact_at_tokens = 250"),
        profiles: profiles(&[profile(
            "auto",
            "automatic_prefix",
            &["tool_calls_per_turn = 0"],
        )]),
        ..Spec::default()
    });
    r.result.unwrap();
    let i = r
        .bodies
        .iter()
        .position(|(_, b)| {
            b["messages"].as_array().unwrap().last().unwrap()["content"]
                .as_str()
                .is_some_and(|c| c.starts_with("Summarise the conversation"))
        })
        .unwrap();
    let reply = acn_harness::wire::assemble(
        acn_harness::context::Dialect::ChatCompletions,
        &r.exchanges[i],
        true,
    )
    .unwrap();
    let summary = reply.text.unwrap();
    assert!(!summary.is_empty());
    let next = r.bodies[i + 1].1["messages"].as_array().unwrap();
    let first_user = next.iter().find(|m| m["role"] == "user").unwrap();
    assert_eq!(
        first_user["content"],
        format!("Summary of earlier conversation:\n{summary}")
    );
}
