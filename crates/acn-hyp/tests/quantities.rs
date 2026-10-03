//! HYP-12: name resolution, selectors, unique enum values, and the quantity table
//! and its rendering.

#![allow(clippy::unwrap_used, clippy::expect_used, clippy::panic)] // CON-19: tests are exempt

mod common;

use acn_hyp::predicate::is_reserved;
use acn_hyp::quantities::{QUANTITIES, get, markdown};
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
    // What hypotheses/p4.toml measures resolves.
    for q in [
        "cached_token_ratio",
        "ttft_p50_ms",
        "ttft_p99_ms",
        "cost_per_success",
        "input_tokens_per_turn",
        "compactions_per_session",
    ] {
        assert!(get(q).is_some(), "{q}");
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
fn selectors_choose_one_arm_fix_each_parameter_once_within_its_domain() {
    let ok = |p: &str| assert!(candidate(&with_predicate(p), "t1").0.is_ok(), "{p}");
    let bad = |p: &str, needle: &str| {
        let e = err(candidate(&with_predicate(p), "t1").0);
        assert!(e.contains(needle), "{p}: {e}");
    };
    ok(
        "cached_token_ratio(fast, knob = true) - cached_token_ratio(control, slow, knob = true) < 1",
    );
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
        "outside its domain",
    );
    bad(
        "cached_token_ratio(knob = 1, fast) < 1",
        "outside its domain",
    );
    bad(
        "cached_token_ratio(medium, knob = true) < 1",
        "not an enum value",
    );
    bad(
        "cached_token_ratio(speed = 1) < 1",
        "not a [varies] parameter",
    );
    // Fixed in one treatment term, free in another.
    bad(
        "cached_token_ratio(fast, knob = true) - cached_token_ratio(knob = true) < 1 at all cells",
        "fixed in one treatment term and free in another",
    );
    // A fix and a bare quantity or per-cell built-in together leave it free.
    bad(
        "max_over_knobs(cached_token_ratio(fast) - effect(cached_token_ratio)) < 1",
        "would leave it free",
    );
    bad(
        "cached_token_ratio(fast) < cached_token_ratio at all cells",
        "would leave it free",
    );
    // With a control term, a parameter is fixed to one value only.
    bad(
        "cached_token_ratio(fast, knob = true) + cached_token_ratio(slow, knob = true) - cached_token_ratio(control) < 1",
        "fixed to one value only",
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
