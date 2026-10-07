//! Plans (SPEC 050 GEN-3, GEN-4): draws come from sub-streams named by what
//! they decide, and a turn's plan is drawn whole, in a fixed order.

#![allow(clippy::unwrap_used, clippy::expect_used, clippy::panic)] // CON-19: tests are exempt

use acn_gen::plan::{self, Chain};
use acn_gen::sheet::{SUBAGENT, Sheet};

fn shipped_text() -> String {
    std::fs::read_to_string(concat!(
        env!("CARGO_MANIFEST_DIR"),
        "/../../workloads/gen/appendix-c.toml"
    ))
    .unwrap()
}

fn sheet(text: &str) -> Sheet {
    Sheet::parse(text, &acn_mockllm::profile::embedded().unwrap()).unwrap()
}

/// Cites: GEN-3
#[test]
fn changing_one_parameter_shifts_no_other_draw() {
    let base = sheet(&shipped_text());
    // Think times are drawn after a session's start and turn count, on the
    // session's stream; turns draw from their own streams.
    let thinker = sheet(&shipped_text().replace(
        "think_time_ns = { quantiles = [[0, 1_000_000_000],",
        "think_time_ns = { quantiles = [[0, 2_000_000_000],",
    ));
    for k in 0..8 {
        let (a, b) = (
            plan::session(&base, 11, k).unwrap(),
            plan::session(&thinker, 11, k).unwrap(),
        );
        assert_eq!((a.start_ns, a.turns), (b.start_ns, b.turns));
        for t in 0..a.turns {
            assert_eq!(
                plan::turn(&base, 11, k, t).unwrap(),
                plan::turn(&thinker, 11, k, t).unwrap()
            );
        }
    }
    // A turn-level parameter leaves every session's own draws as they were.
    let answers = sheet(&shipped_text().replace(
        "answer_tokens = { quantiles = [[0, 16],",
        "answer_tokens = { quantiles = [[0, 17],",
    ));
    for k in 0..8 {
        assert_eq!(
            plan::session(&base, 11, k).unwrap(),
            plan::session(&answers, 11, k).unwrap()
        );
    }
    // One turn's plan does not depend on another's.
    assert_ne!(
        plan::turn(&base, 11, 0, 0).unwrap(),
        plan::turn(&base, 11, 0, 1).unwrap()
    );
    assert_eq!(
        plan::turn(&base, 11, 0, 1).unwrap(),
        plan::turn(&base, 11, 0, 1).unwrap()
    );
}

/// A chain drawn by hand in GEN-4's order.
fn by_hand(s: &Sheet, rng: &mut rand_chacha::ChaCha20Rng, fans: bool) -> (Chain, u64) {
    let user_tokens = s.user_tokens.draw(rng);
    let length = s.chain_length.draw(rng);
    let width = if fans { s.fanout_width.draw(rng) } else { 0 };
    let mut tools = Vec::new();
    for i in 0..length {
        if i == 0 && width > 0 {
            tools.push(plan::ToolStep {
                class: SUBAGENT,
                duration_ns: 0,
                result_tokens: 0,
            });
            continue;
        }
        let class = s.classes[s.tool_class.draw(rng) as usize];
        let duration_ns = s.tool_duration_ns[class].draw(rng);
        let result_tokens = s.tool_result_tokens[class].draw(rng);
        tools.push(plan::ToolStep {
            class,
            duration_ns,
            result_tokens,
        });
    }
    let answer_tokens = s.answer_tokens.draw(rng);
    (
        Chain {
            user_tokens,
            tools,
            answer_tokens,
        },
        width,
    )
}

/// Cites: GEN-4, GEN-12
#[test]
fn a_turns_plan_is_drawn_whole_in_gen_4_order() {
    // Every turn fans out, two sub-agents, so the order covers sub-chains.
    let s = sheet(&shipped_text().replace(
        "fanout_width = { weighted = [[0, 85], [2, 10], [4, 5]] }",
        "fanout_width = { const = 2 }",
    ));
    for (k, t) in [(0, 0), (2, 3), (7, 1)] {
        let p = plan::turn(&s, 5, k, t).unwrap();
        let mut rng = acn_trace::identity::substream_rng(5, &format!("gen.plan.{k}.{t}")).unwrap();
        let (main, width) = by_hand(&s, &mut rng, true);
        assert_eq!(p.main, main);
        assert_eq!(width, 2);
        let children: Vec<Chain> = (0..width).map(|_| by_hand(&s, &mut rng, false).0).collect();
        assert_eq!(p.children, children);
        // The spawn is the first tool call; sub-agents never spawn (HAR-5).
        assert_eq!(p.main.tools[0].class, SUBAGENT);
        assert!(
            p.children
                .iter()
                .flat_map(|c| &c.tools)
                .all(|x| x.class != SUBAGENT)
        );
    }
}

/// Cites: GEN-13
#[test]
fn text_is_four_lowercase_letters_per_token_from_its_own_stream() {
    let mut a = plan::text_stream(5, 0, 0, 1).unwrap();
    let w = acn_gen::text::words(&mut a, 25);
    assert_eq!(w.len(), 100);
    assert!(w.bytes().all(|b| b.is_ascii_lowercase()));
    // Another lineage of the same turn has its own stream.
    let mut b = plan::text_stream(5, 0, 0, 2).unwrap();
    assert_ne!(w, acn_gen::text::words(&mut b, 25));
    let mut s = plan::system_stream(5).unwrap();
    assert_eq!(acn_gen::text::words(&mut s, 3).len(), 12);
}

/// Cites: GEN-3
#[test]
fn a_sessions_draws_are_its_start_turns_and_think_times_in_order() {
    let s = sheet(&shipped_text());
    for k in [0, 3, 7] {
        let p = plan::session(&s, 9, k).unwrap();
        let mut rng = acn_trace::identity::substream_rng(9, &format!("gen.session.{k}")).unwrap();
        assert_eq!(p.start_ns, s.session_start_ns.draw(&mut rng));
        assert_eq!(p.turns, s.turns_per_session.draw(&mut rng));
        let think: Vec<u64> = (1..p.turns)
            .map(|_| s.think_time_ns.draw(&mut rng))
            .collect();
        assert_eq!(p.think_ns, think);
    }
}

/// Cites: GEN-4
#[test]
fn a_turn_that_does_not_fan_out_is_drawn_in_gen_4_order_too() {
    let s = sheet(&shipped_text());
    let mut checked = 0;
    for k in 0..4 {
        for t in 0..4 {
            let p = plan::turn(&s, 5, k, t).unwrap();
            if !p.children.is_empty() {
                continue;
            }
            let mut rng =
                acn_trace::identity::substream_rng(5, &format!("gen.plan.{k}.{t}")).unwrap();
            let (main, width) = by_hand(&s, &mut rng, true);
            assert_eq!(width, 0);
            assert_eq!(p.main, main);
            checked += 1;
        }
    }
    assert!(checked > 0, "no turn without fan-out among those drawn");
}
