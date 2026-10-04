//! HYP-12: name resolution, selectors normalised to one spelling, unique enum
//! values, the quantity table and its rendering, every formula per replicate
//! (then the mean over replicates), and the price table.

#![allow(clippy::unwrap_used, clippy::expect_used, clippy::panic)] // CON-19: tests are exempt

mod common;

use acn_hyp::predicate::is_reserved;
use acn_hyp::quantities::{
    Call, PRICES, QUANTITIES, Replicate, Session, Turn, get, markdown, prices, value,
};
use acn_hyp::slice::{Arm, Values};
use common::{BASE, candidate, err, with_predicate};

/// Cites: HYP-12
#[test]
fn the_table_names_each_quantity_once_with_a_unit_and_a_formula() {
    let mut names = std::collections::BTreeSet::new();
    for q in QUANTITIES {
        assert!(names.insert(q.name), "{} twice", q.name);
        assert!(
            acn_hyp::predicate::is_ident(q.name) && !is_reserved(q.name),
            "{}",
            q.name
        );
        assert!(!q.unit.is_empty() && !q.formula.is_empty() && !q.source.is_empty());
        assert_eq!(get(q.name), Some(q));
    }
    for q in [
        "cached_token_ratio",
        "ttft_p50_ms",
        "ttft_p99_ms",
        "cost_per_success",
        "input_tokens_per_turn",
        "compactions_per_session",
    ] {
        assert!(
            get(q).is_some(),
            "{q}: what hypotheses/p4.toml measures resolves"
        );
    }
    let page = markdown();
    for q in QUANTITIES {
        assert!(page.contains(&format!("| `{}` | {} |", q.name, q.unit)));
    }
    let generated = std::fs::read_to_string(concat!(
        env!("CARGO_MANIFEST_DIR"),
        "/../../docs/generated/quantities.md"
    ))
    .unwrap();
    assert!(
        generated.ends_with(&page),
        "docs-inventory renders the table"
    );
}

/// Cites: HYP-12
#[test]
fn selectors_choose_one_arm_and_fix_each_parameter_once_to_a_runnable_value() {
    let ok = |p: &str| assert!(candidate(&with_predicate(p), "t1").0.is_ok(), "{p}");
    let bad = |p: &str, needle: &str| {
        let e = err(candidate(&with_predicate(p), "t1").0);
        assert!(e.contains(needle), "{p}: {e}");
    };
    ok("cached_token_ratio(fast, knob = true) - cached_token_ratio(control) < 1");
    bad(
        "cached_token_ratio(control, treatment, fast, knob = true) < 1",
        "names an arm twice",
    );
    bad(
        "cached_token_ratio(fast, mode = slow, knob = true) < 1",
        "fixes `mode` twice",
    );
    bad(
        "cached_token_ratio(mode = medium, knob = true) < 1",
        "not a value a run takes",
    );
    bad(
        "cached_token_ratio(knob = 1, fast) < 1",
        "not a value a run takes",
    );
    bad(
        "cached_token_ratio(medium, knob = true) < 1",
        "not an enum value",
    );
    bad(
        "cached_token_ratio(speed = 1) < 1",
        "not a [varies] parameter",
    );
    // Fixed in one term, free in a treatment term.
    bad(
        "cached_token_ratio(fast, knob = true) - cached_token_ratio(knob = true) < 1 at all cells",
        "free in a treatment term",
    );
    // A fix and a bare quantity or per-cell built-in together leave it free,
    // whichever arm the fix is in.
    bad(
        "max_over_knobs(cached_token_ratio(fast) - effect(cached_token_ratio)) < 1",
        "would leave it free",
    );
    bad(
        "cached_token_ratio(fast) < cached_token_ratio at all cells",
        "would leave it free",
    );
    bad(
        "cached_token_ratio(control, fast) > cached_token_ratio + 0.1 at all cells",
        "would leave it free",
    );
    // With a control term, a parameter takes one value across every term,
    // the control's included.
    bad(
        "cached_token_ratio(fast, knob = true) + cached_token_ratio(slow, knob = true) - cached_token_ratio(control) < 1",
        "one value only",
    );
    bad(
        "cached_token_ratio(control, slow, knob = true) > cached_token_ratio(fast, knob = true)",
        "one value only",
    );
    // A value outside a grid's levels is never run.
    let levels = BASE.replace(
        "mode = { kind = \"enum\", values = [\"fast\", \"slow\"] }",
        "mode = { kind = \"enum\", values = [\"fast\", \"slow\"] }\nrtt = { kind = \"int_range\", min = 0, max = 300, levels = [50, 300] }\nloss = { kind = \"range\", min = 0, max = 1, levels = [0.5] }",
    );
    let with = |p: &str| {
        levels.replace(
        "predicate = \"max_over_knobs(abs(effect(cached_token_ratio))) < noise_floor(cached_token_ratio, control)\"",
        &format!("predicate = {}", common::toml_string(p)),
    )
    };
    assert!(
        candidate(
            &with(
                "max_over_knobs(cached_token_ratio(rtt = 50, loss = 0.5, fast, knob = true)) < 1"
            ),
            "t1"
        )
        .0
        .is_ok()
    );
    for p in [
        "max_over_knobs(cached_token_ratio(rtt = 100, loss = 0.5, fast, knob = true)) < 1",
        "max_over_knobs(cached_token_ratio(rtt = 50.5, loss = 0.5, fast, knob = true)) < 1",
        "max_over_knobs(cached_token_ratio(rtt = 50, loss = 0.25, fast, knob = true)) < 1",
    ] {
        assert!(
            err(candidate(&with(p), "t1").0).contains("not a value a run takes"),
            "{p}"
        );
    }
    // One term, two spellings: normalised to one, so lint sees one slot.
    let a = candidate(
        &with_predicate("max_over_knobs(cached_token_ratio(fast, knob = true)) < 1"),
        "t1",
    )
    .0
    .unwrap();
    let b = candidate(
        &with_predicate("max_over_knobs(cached_token_ratio(knob = true, mode = fast)) < 1"),
        "t1",
    )
    .0
    .unwrap();
    assert_eq!(a.predicate(), b.predicate());
    assert_eq!(
        a.predicate().to_string(),
        "(max_over_knobs(cached_token_ratio(knob = true, mode = fast)) < 1)"
    );
    // A selector never fixes a non-pooled parameter.
    let np = BASE
        .replace(
            "mode = { kind = \"enum\", values = [\"fast\", \"slow\"] }",
            "mode = { kind = \"enum\", values = [\"fast\", \"slow\"] }\nprovider = { kind = \"enum\", values = [\"a\", \"b\"] }",
        )
        .replace(
            "predicate = \"max_over_knobs(abs(effect(cached_token_ratio))) < noise_floor(cached_token_ratio, control)\"",
            "predicate = \"max_over_knobs(cached_token_ratio(a, fast)) < 1\"",
        );
    assert!(err(candidate(&np, "t1").0).contains("not pooled"));
}

fn call(sid: u8, input: i64, read: i64, write: i64, out: i64, ttft_ms: i64) -> Call {
    Call {
        session_id: [sid; 8],
        input_tokens: Some(input),
        cache_read_tokens: Some(read),
        cache_write_tokens: Some(write),
        output_tokens: Some(out),
        ttft_ns: Some(ttft_ms * 1_000_000),
    }
}

fn turn(sid: u8, outcome: &str, compaction: &str) -> Turn {
    Turn {
        session_id: [sid; 8],
        outcome: outcome.into(),
        compaction: compaction.into(),
    }
}

/// Two sessions, three turns (two successes, one compaction), four calls.
fn replicate(provider: &str) -> Replicate {
    Replicate {
        sessions: vec![
            Session { session_id: [1; 8] },
            Session { session_id: [2; 8] },
        ],
        turns: vec![
            turn(1, "success", "none"),
            turn(1, "failure", "window_full"),
            turn(2, "success", "none"),
        ],
        calls: vec![
            call(1, 1000, 0, 1000, 100, 400),
            call(1, 1200, 1000, 0, 50, 100),
            call(2, 1000, 900, 0, 10, 200),
            call(2, 800, 0, 0, 40, 300),
        ],
        price_key: provider.into(),
    }
}

/// Cites: HYP-12
#[test]
fn every_quantity_has_a_formula_over_one_replicates_rows() {
    let r = replicate("anthropic");
    for q in QUANTITIES {
        assert!(value(q.name, &r).is_some(), "{} has no formula", q.name);
    }
    assert_eq!(value("no_such_quantity", &r), None);
    // 1900 read of 4000 input.
    assert_eq!(value("cached_token_ratio", &r), Some(1900.0 / 4000.0));
    // ttft 100, 200, 300, 400 ms: nearest rank ceil(0.5 · 4) = 2 and ceil(0.99 · 4) = 4.
    assert_eq!(value("ttft_p50_ms", &r), Some(200.0));
    assert_eq!(value("ttft_p99_ms", &r), Some(400.0));
    assert_eq!(value("input_tokens_per_turn", &r), Some(4000.0 / 3.0));
    assert_eq!(value("compactions_per_session", &r), Some(0.5));
    let mut both = replicate("anthropic");
    both.turns[0].compaction = "read_cost_threshold".into();
    assert_eq!(
        value("compactions_per_session", &both),
        Some(1.0),
        "every kind of compaction counts"
    );
    // Anthropic weights 1, 0.1, 1.25, 5: uncached 0 + 200 + 100 + 800 = 1100,
    // read 1900 · 0.1, write 1000 · 1.25, output 200 · 5; over 2 successes.
    let cost = 1100.0 + 190.0 + 1250.0 + 1000.0;
    assert_eq!(value("cost_per_success", &r), Some(cost / 2.0));
    let o = replicate("openai");
    let cost = 1100.0 + 190.0 + 1000.0 + 1600.0;
    assert_eq!(value("cost_per_success", &o), Some(cost / 2.0));
}

/// Cites: HYP-12, HYP-11
#[test]
fn a_formula_is_undefined_where_its_data_are_missing_and_never_a_partial_total() {
    let mut r = replicate("anthropic");
    r.calls[1].input_tokens = None;
    assert_eq!(value("input_tokens_per_turn", &r), None, "a partial sum");
    assert_eq!(value("cost_per_success", &r), None, "a partial cost");
    assert_eq!(
        value("cached_token_ratio", &r),
        None,
        "a call is never dropped"
    );
    let mut r = replicate("openai");
    r.calls[2].cache_read_tokens = None;
    assert_eq!(
        value("cached_token_ratio", &r),
        None,
        "an unreported cache count"
    );
    // A call that never produced a token is not dropped from the percentile:
    // dropping it would make a treatment that times out look faster.
    let mut r = replicate("anthropic");
    r.calls[3].ttft_ns = None;
    assert_eq!(value("ttft_p50_ms", &r), None);
    assert_eq!(value("ttft_p99_ms", &r), None);
    assert_eq!(
        value("cost_per_success", &replicate("vllm")),
        None,
        "no price row"
    );
    let mut r = replicate("anthropic");
    for t in &mut r.turns {
        t.outcome = "timeout".into();
    }
    assert_eq!(
        value("cost_per_success", &r),
        None,
        "no success: division by zero"
    );
    let mut r = replicate("anthropic");
    r.calls[0].cache_read_tokens = Some(2000);
    assert_eq!(value("cost_per_success", &r), None, "more cached than sent");
    let mut r = replicate("anthropic");
    for c in &mut r.calls {
        c.ttft_ns = None;
    }
    assert_eq!(value("ttft_p50_ms", &r), None);
    let empty = Replicate::default();
    for q in QUANTITIES {
        assert_eq!(value(q.name, &empty), None, "{} over no rows", q.name);
    }
}

/// Cites: HYP-12
#[test]
fn an_arm_is_the_mean_over_replicates_of_the_per_replicate_values() {
    // Two replicates with cached ratios 1/2 and 1/4 over very different volumes:
    // the arm is 0.375, the mean of the ratios, not 0.26 from pooled tokens.
    let one = |read: i64, input: i64| Replicate {
        calls: vec![Call {
            session_id: [1; 8],
            input_tokens: Some(input),
            cache_read_tokens: Some(read),
            ..Call::default()
        }],
        ..Replicate::default()
    };
    let vals = |r: &Replicate| {
        Some(Values::from([(
            "cached_token_ratio".to_owned(),
            value("cached_token_ratio", r),
        )]))
    };
    let arm = Arm {
        replicates: vec![vals(&one(5, 10)), vals(&one(250, 1000))],
    };
    assert_eq!(arm.mean("cached_token_ratio"), Some(0.375));
}

/// Cites: HYP-12
#[test]
fn the_price_table_names_each_provider_once_in_input_token_units() {
    let mut seen = std::collections::BTreeSet::new();
    for p in PRICES {
        assert!(seen.insert(p.provider), "{} twice", p.provider);
        assert_eq!(
            p.input, 1.0,
            "{}: the unit is one uncached input token",
            p.provider
        );
        for w in [p.cache_read, p.cache_write, p.output] {
            assert!(w.is_finite() && w >= 0.0);
        }
        assert!(
            p.cache_read < p.input,
            "{}: a cache read costs less",
            p.provider
        );
        assert!(!p.reference.is_empty());
        assert_eq!(prices(p.provider), Some(p));
        assert!(markdown().contains(&format!("| `{}` |", p.provider)));
    }
    // p4's provider values: the two with list prices have rows.
    assert!(prices("anthropic").is_some() && prices("openai").is_some());
    assert!(prices("vllm").is_none() && prices("sglang").is_none());
    assert!(
        prices("anthropic-bedrock").is_none() && prices("").is_none(),
        "exact names only"
    );
}
