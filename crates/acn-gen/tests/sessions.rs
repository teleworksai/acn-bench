//! The generator's sessions (SPEC 050 GEN-10 to GEN-15) as the bundle records
//! them: what was planned is what was sent.

#![allow(clippy::unwrap_used, clippy::expect_used, clippy::panic)] // CON-19: tests are exempt

mod common;

use acn_gen::plan;
use acn_gen::sheet::{SUBAGENT, Sheet};
use acn_trace::identity::replicate_seed;
use common::{SMALL, children, config, int, read, run_ok, spans, text};

fn sheet(text: &str) -> Sheet {
    Sheet::parse(text, &acn_mockllm::profile::embedded().unwrap()).unwrap()
}

/// Cites: GEN-10, GEN-11, GEN-12, GEN-15, GEN-30
#[test]
fn sessions_turns_chains_and_fan_out_are_sent_as_planned() {
    let dir = tempfile::tempdir().unwrap();
    let w = run_ok(&config(dir.path(), SMALL));
    let t = read(&w.written.dir);
    let s = sheet(SMALL);
    let rseed = replicate_seed(7, 0).unwrap();
    let sessions = spans(&t, "acn.session");
    assert_eq!(sessions.len(), 3);
    assert_eq!(w.sessions, 3);
    // Every session starts at its drawn start (GEN-10).
    let mut starts: Vec<i64> = sessions.iter().map(|x| x.start_ns).collect();
    starts.sort_unstable();
    let mut planned: Vec<i64> = (0..3)
        .map(|k| plan::session(&s, rseed, k).unwrap().start_ns as i64)
        .collect();
    planned.sort_unstable();
    let origin = starts[0] - planned[0];
    assert_eq!(
        starts.iter().map(|x| x - origin).collect::<Vec<_>>(),
        planned
    );
    let mut calls = 0;
    for session in &sessions {
        let k = (0..3)
            .find(|k| {
                plan::session(&s, rseed, *k).unwrap().start_ns as i64 == session.start_ns - origin
            })
            .unwrap();
        let sp = plan::session(&s, rseed, k).unwrap();
        let turns = children(&t, session, "acn.turn");
        assert_eq!(turns.len() as u64, sp.turns);
        for (ti, turn) in turns.iter().enumerate() {
            assert_eq!(int(turn, "acn.turn.index"), Some(ti as i64));
            assert_eq!(text(turn, "acn.turn.outcome"), Some("success"));
            // Think time from the previous turn's end (GEN-10).
            if ti > 0 {
                assert_eq!(
                    turn.start_ns - turns[ti - 1].end_ns,
                    sp.think_ns[ti - 1] as i64
                );
            }
            let p = plan::turn(&s, rseed, k, ti as u64).unwrap();
            // The chain: one call per tool step, then the answer (GEN-11,
            // GEN-30), with call indices from 0.
            let chats = children(&t, turn, "chat");
            assert_eq!(chats.len(), p.main.tools.len() + 1);
            let idx: Vec<i64> = chats
                .iter()
                .map(|c| int(c, "acn.call.index").unwrap())
                .collect();
            assert_eq!(idx, (0..chats.len() as i64).collect::<Vec<_>>());
            calls += chats.len();
            // The tools run are the classes planned, in order.
            let tools = children(&t, turn, "execute_tool");
            let classes: Vec<&str> = tools
                .iter()
                .map(|x| text(x, "acn.tool.class").unwrap())
                .collect();
            let want: Vec<&str> = p.main.tools.iter().map(|x| x.class).collect();
            assert_eq!(classes, want);
            for (x, step) in tools.iter().zip(&p.main.tools) {
                assert_eq!(
                    text(x, "gen_ai.tool.name"),
                    Some(format!("gen_{}", step.class).as_str())
                );
            }
            // Fan-out (GEN-12): the first tool call spawns its sub-agents.
            if let Some(spawn) = tools
                .first()
                .filter(|x| text(x, "acn.tool.class") == Some(SUBAGENT))
            {
                let agents = children(&t, spawn, "invoke_agent");
                assert_eq!(agents.len(), p.children.len());
                // Sub-agents start at one instant, so their spans do not keep
                // spawn order: compare their chains as a set of lengths.
                let mut got = Vec::new();
                for a in &agents {
                    assert_eq!(int(a, "acn.fanout.width"), Some(p.children.len() as i64));
                    assert_eq!(int(a, "acn.fanout.parent_call"), Some(0));
                    let sub = children(&t, a, "chat");
                    got.push(sub.len());
                    calls += sub.len();
                }
                let mut want: Vec<usize> = p.children.iter().map(|c| c.tools.len() + 1).collect();
                got.sort_unstable();
                want.sort_unstable();
                assert_eq!(got, want);
            } else {
                assert!(p.children.is_empty());
            }
        }
    }
    assert_eq!(w.calls, calls as u64);
    // A fan-out happened somewhere, so it was exercised.
    assert!(!spans(&t, "invoke_agent").is_empty(), "no fan-out drawn");
}

/// Cites: GEN-11, GEN-13
#[test]
fn every_call_extends_the_last_so_the_prefix_is_cached() {
    let dir = tempfile::tempdir().unwrap();
    let w = run_ok(&config(dir.path(), SMALL));
    let t = read(&w.written.dir);
    // On `mock-agentic`, 300 system tokens and the tools are below the
    // profile's minimum, but a session's later calls carry its whole
    // conversation and are cached from its earlier calls (MLM-10).
    let cached = spans(&t, "chat")
        .iter()
        .filter(|c| int(c, "acn.cache.read_tokens").unwrap_or(0) > 0)
        .count();
    assert!(cached > 0, "no call read a cached prefix");
    for c in spans(&t, "chat") {
        let input = int(c, "acn.call.input_tokens").unwrap();
        let new = int(c, "acn.call.new_input_tokens").unwrap();
        assert!(new <= input);
    }
}

/// Cites: GEN-14
#[test]
fn a_long_context_is_compacted_as_har_4_compacts() {
    let dir = tempfile::tempdir().unwrap();
    let small = SMALL.replace("compact_at_tokens = 0", "compact_at_tokens = 900");
    let w = run_ok(&config(dir.path(), &small));
    let t = read(&w.written.dir);
    let compacted: Vec<_> = spans(&t, "acn.turn")
        .into_iter()
        .filter(|x| text(x, "acn.turn.compaction") == Some("window_full"))
        .collect();
    assert!(!compacted.is_empty(), "no turn compacted");
    // A compacting turn makes one summary call more than its plan.
    let s = sheet(&small);
    let rseed = replicate_seed(7, 0).unwrap();
    let sessions = spans(&t, "acn.session");
    let starts: Vec<i64> = (0..3)
        .map(|k| plan::session(&s, rseed, k).unwrap().start_ns as i64)
        .collect();
    let origin = sessions.iter().map(|x| x.start_ns).min().unwrap() - starts.iter().min().unwrap();
    for session in sessions {
        let k = (0..3)
            .find(|k| starts[*k as usize] == session.start_ns - origin)
            .unwrap();
        for (ti, turn) in children(&t, session, "acn.turn").iter().enumerate() {
            let p = plan::turn(&s, rseed, k, ti as u64).unwrap();
            let chats = children(&t, turn, "chat").len();
            if text(turn, "acn.turn.compaction") == Some("window_full") {
                assert!(chats > p.main.tools.len() + 1);
            } else {
                assert_eq!(chats, p.main.tools.len() + 1);
            }
        }
    }
}

/// Cites: GEN-15, HAR-24
#[test]
fn a_call_that_fails_aborts_its_turn_and_the_session_goes_on() {
    let dir = tempfile::tempdir().unwrap();
    let mut cfg = config(dir.path(), SMALL);
    // Every call answered with a 500, and no retry (HAR-24).
    cfg.profiles = Some(
        acn_mockllm::profile::Profiles::parse(
            &acn_mockllm::profile::PROFILES_TOML
                .replace("fault_500_ppm = 0", "fault_500_ppm = 1000000"),
        )
        .unwrap(),
    );
    cfg.opts.max_retries = 0;
    let w = run_ok(&cfg);
    let t = read(&w.written.dir);
    let s = sheet(SMALL);
    let rseed = replicate_seed(7, 0).unwrap();
    let turns = spans(&t, "acn.turn");
    let planned: u64 = (0..3)
        .map(|k| plan::session(&s, rseed, k).unwrap().turns)
        .sum();
    // Every turn ran, each aborted at its first call.
    assert_eq!(turns.len() as u64, planned);
    for turn in turns {
        assert_eq!(text(turn, "acn.turn.outcome"), Some("aborted"));
        assert_eq!(children(&t, turn, "chat").len(), 1);
    }
}
