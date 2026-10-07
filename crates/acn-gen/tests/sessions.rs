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
    // Sessions are told apart by their starts: the draws give distinct ones.
    assert!(planned.windows(2).all(|w| w[0] < w[1]), "{planned:?}");
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

/// Profiles that cache from a small prefix, so every prefix shows.
fn low_minimum() -> acn_mockllm::profile::Profiles {
    acn_mockllm::profile::Profiles::parse(
        &acn_mockllm::profile::PROFILES_TOML
            .replace("min_cacheable_tokens = 1024", "min_cacheable_tokens = 32")
            .replace("increment_tokens = 128", "increment_tokens = 16"),
    )
    .unwrap()
}

/// Cites: GEN-11, GEN-13
#[test]
fn within_a_turn_every_call_extends_the_last() {
    let dir = tempfile::tempdir().unwrap();
    let mut cfg = config(dir.path(), SMALL);
    cfg.profiles = Some(low_minimum());
    let w = run_ok(&cfg);
    let t = read(&w.written.dir);
    // Every main-lineage call of a turn carries the same tools, and its
    // messages extend the previous call's, so it reads the previous call's
    // whole input from the cache, less at most one increment (MLM-10). The
    // system prompt's timestamp changes with each turn (HAR-11, ADR-38), so
    // turns do not share a cached prefix past it.
    let mut pairs = 0;
    for turn in spans(&t, "acn.turn") {
        let chats = children(&t, turn, "chat");
        for w in chats.windows(2) {
            let prev = int(w[0], "acn.call.input_tokens").unwrap();
            let read = int(w[1], "acn.cache.read_tokens").unwrap_or(0);
            assert!(read >= prev - 16, "read {read} after an input of {prev}");
            pairs += 1;
        }
    }
    assert!(pairs > 0);
}

/// Cites: GEN-12, GEN-13
#[test]
fn sub_agents_share_the_turns_prefix_and_never_spawn_or_compact() {
    let dir = tempfile::tempdir().unwrap();
    let fans = SMALL
        .replace(
            "fanout_width = { weighted = [[0, 2], [2, 1]] }",
            "fanout_width = { const = 2 }",
        )
        .replace("compact_at_tokens = 0", "compact_at_tokens = 900");
    let w = run_ok(&config(dir.path(), &fans));
    let t = read(&w.written.dir);
    let agents = spans(&t, "invoke_agent");
    assert!(!agents.is_empty());
    // The mock reads the tools first (MLM-10), and a sub-agent's tools are
    // its parent's without `gen_subagent` (GEN-12): the prefix it shares with
    // the spawning call is the tools before that one, the same for every
    // sub-agent of the sheet (TRC-14).
    let shared: std::collections::BTreeSet<i64> = agents
        .iter()
        .map(|a| int(a, "acn.fanout.shared_prefix_tokens").unwrap())
        .collect();
    assert_eq!(shared.len(), 1, "{shared:?}");
    assert!(shared.iter().all(|x| *x > 0));
    for a in &agents {
        // Its calls never spawn: no `subagent` tool runs under it.
        for x in children(&t, a, "execute_tool") {
            assert_ne!(text(x, "acn.tool.class"), Some(SUBAGENT));
        }
    }
    // Each sub-agent span names its turn.
    for turn in spans(&t, "acn.turn") {
        for x in children(&t, turn, "execute_tool") {
            for a in children(&t, x, "invoke_agent") {
                assert_eq!(
                    int(a, "acn.fanout.parent_turn"),
                    int(turn, "acn.turn.index")
                );
            }
        }
    }
}

/// Cites: GEN-10, GEN-21
#[test]
fn replicates_each_run_every_session_and_the_counts_add_up() {
    let dir = tempfile::tempdir().unwrap();
    let mut cfg = config(dir.path(), SMALL);
    cfg.replicates = 2;
    let w = run_ok(&cfg);
    let t = read(&w.written.dir);
    assert_eq!(spans(&t, "acn.session").len(), 6);
    assert_eq!(w.sessions, 6);
    assert_eq!(w.calls, spans(&t, "chat").len() as u64);
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

/// Cites: GEN-15, GEN-12
#[test]
fn a_sub_agent_that_fails_aborts_its_turn_and_the_session_goes_on() {
    let dir = tempfile::tempdir().unwrap();
    let fans = SMALL.replace(
        "fanout_width = { weighted = [[0, 2], [2, 1]] }",
        "fanout_width = { const = 2 }",
    );
    let mut cfg = config(dir.path(), &fans);
    // A fifth of calls answered with a 500, and no retry.
    cfg.profiles = Some(
        acn_mockllm::profile::Profiles::parse(
            &acn_mockllm::profile::PROFILES_TOML
                .replace("fault_500_ppm = 0", "fault_500_ppm = 200000"),
        )
        .unwrap(),
    );
    cfg.opts.max_retries = 0;
    let w = run_ok(&cfg);
    let t = read(&w.written.dir);
    let s = sheet(&fans);
    let rseed = replicate_seed(7, 0).unwrap();
    let planned: u64 = (0..3)
        .map(|k| plan::session(&s, rseed, k).unwrap().turns)
        .sum();
    let turns = spans(&t, "acn.turn");
    // Every turn ran, whatever failed before it.
    assert_eq!(turns.len() as u64, planned);
    // A turn whose spawning call succeeded and one of whose sub-agents
    // failed: aborted, its spawn and its sub-agents' spans ended.
    let mut seen = 0;
    for turn in turns {
        let Some(spawn) = children(&t, turn, "execute_tool")
            .into_iter()
            .find(|x| text(x, "acn.tool.class") == Some(SUBAGENT))
        else {
            continue;
        };
        let agents = children(&t, spawn, "invoke_agent");
        let failed = agents.iter().any(|a| {
            children(&t, a, "chat")
                .iter()
                .any(|c| text(c, "acn.call.error_class").is_some())
        });
        if failed {
            assert_eq!(text(turn, "acn.turn.outcome"), Some("aborted"));
            assert!(spawn.end_ns >= spawn.start_ns);
            assert!(
                agents
                    .iter()
                    .all(|a| a.end_ns >= a.start_ns && a.end_ns <= spawn.end_ns)
            );
            // The turn stops at the spawn: no call after it.
            let after = children(&t, turn, "chat")
                .iter()
                .filter(|c| c.start_ns > spawn.end_ns)
                .count();
            assert_eq!(after, 0);
            seen += 1;
        }
    }
    assert!(seen > 0, "no sub-agent failed under this seed");
}
