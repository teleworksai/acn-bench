//! HYP-10, HYP-11, HYP-13, HYP-14: the grammar and the precedence of `at`, signed
//! bounds, the counters-only guard, the three types and the unit check, the
//! built-ins, per-cell versus slice-level, and ambiguity rejection; golden parse
//! trees as fully parenthesised renderings.

#![allow(clippy::unwrap_used, clippy::expect_used, clippy::panic)] // CON-19: tests are exempt

mod common;

use acn_hyp::predicate::parse;
use common::{BASE, candidate, err, with_predicate};

fn tree(p: &str) -> String {
    parse(p).unwrap().to_string()
}

/// Cites: HYP-10
#[test]
fn the_grammar_and_its_precedence_parse_to_golden_trees() {
    for (src, golden) in [
        (
            "a < 1 or b > 2 at all x <= 1",
            "(((a < 1) or (b > 2)) at all x <= 1)",
        ),
        ("not a < 1 and b < 2", "((not (a < 1)) and (b < 2))"),
        ("a + b * c - d / e < 1", "(((a + (b * c)) - (d / e)) < 1)"),
        ("-a < -1", "((-a) < (-1))"),
        ("a < 1 at any x >= -3.5", "((a < 1) at any x >= -3.5)"),
        ("a < 1 at all cells", "((a < 1) at all cells)"),
        (
            "q(control) < q(p = -2, fast, treatment)",
            "(q[control] < q[p = -2, fast, treatment])",
        ),
        ("ci_low(q, ci = 0.9) > 0", "(ci_low(q, ci = 0.9) > 0)"),
        (
            "noise_floor(q, control) > 1e-3",
            "(noise_floor(q, control) > 0.001)",
        ),
        (
            "replicates < 20 or providers_reported < 2",
            "((replicates < 20) or (providers_reported < 2))",
        ),
        ("(a < 1)", "(a < 1)"),
    ] {
        assert_eq!(tree(src), golden, "{src}");
    }
    for (src, needle) in [
        ("a < 1 < 2", "do not chain"),
        ("a <", "expected a value"),
        ("a < 1 at most x", "`all` or `any`"),
        ("a < 1 at all x", "expected a comparison"),
        ("a < 1.", "followed by digits"),
        ("a < 1e", "exponent"),
        ("a < 1 )", "unexpected `)`"),
        ("A < 1", "unexpected character"),
        ("and < 1", "cannot stand here"),
        ("cells(x) < 1", "reserved"),
        ("q(and) < 1", "reserved"),
    ] {
        let e = parse(src).unwrap_err().to_string();
        assert!(e.contains(needle), "{src}: {e}");
    }
}

/// Cites: HYP-10
#[test]
fn the_guard_reads_counters_and_numbers_only() {
    let guard = |g: &str| {
        BASE.replace(
            "[expected]",
            &format!("inconclusive_if = {}\n\n[expected]", common::toml_string(g)),
        )
    };
    assert!(
        candidate(&guard("replicates < 20 or not replicates >= 4"), "t1")
            .0
            .is_ok()
    );
    for (g, needle) in [
        ("cached_token_ratio < 1", "not a quantity"),
        ("max_over_knobs(1) < 1", "not a built-in"),
        ("replicates + 1 < 20", "not arithmetic"),
        ("replicates < 1 at all cells", "not an `at` clause"),
        ("providers_reported < 2", "no `provider` parameter"),
        ("replicates", "it must be a boolean"),
    ] {
        let e = err(candidate(&guard(g), "t1").0);
        assert!(e.contains(needle), "{g}: {e}");
        assert!(e.contains("falsifier.inconclusive_if"), "{e}");
    }
    let e = err(candidate(
        &with_predicate("max_over_knobs(cached_token_ratio) < providers_reported"),
        "t1",
    )
    .0);
    assert!(e.contains("may not read `providers_reported`"), "{e}");
    assert!(
        candidate(
            &with_predicate("max_over_knobs(cached_token_ratio) < replicates"),
            "t1"
        )
        .0
        .is_ok()
    );
}

/// Cites: HYP-11
#[test]
fn types_and_units_are_checked_without_implicit_conversion() {
    for (p, needle) in [
        (
            "max_over_knobs(cached_token_ratio)",
            "a falsifier is a boolean",
        ),
        (
            "max_over_knobs(cached_token_ratio < 1) < 1",
            "a boolean where a number is needed",
        ),
        (
            "max_over_knobs(cached_token_ratio) and 1 < 2",
            "a number where a boolean is needed",
        ),
        (
            "max_over_knobs(ttft_p50_ms + cached_token_ratio) < 1",
            "unit mismatch",
        ),
        (
            "max_over_knobs(ttft_p50_ms) < max_over_knobs(cached_token_ratio)",
            "unit mismatch",
        ),
        (
            "max_over_knobs(min(ttft_p50_ms, cached_token_ratio)) < 1",
            "unit mismatch",
        ),
    ] {
        let e = err(candidate(&with_predicate(p), "t1").0);
        assert!(e.contains(needle), "{p}: {e}");
    }
    for p in [
        "max_over_knobs(ttft_p50_ms + ttft_p99_ms) < 1",
        "max_over_knobs(ttft_p50_ms * cached_token_ratio) < 1",
        "max_over_knobs(ttft_p50_ms / ttft_p99_ms) < 0.5",
        "max_over_knobs(rel_effect(ttft_p50_ms)) < 0.1",
    ] {
        assert!(candidate(&with_predicate(p), "t1").0.is_ok(), "{p}");
    }
}

/// Cites: HYP-13
#[test]
fn the_built_ins_have_their_signatures_and_no_others_exist() {
    for (p, needle) in [
        (
            "max_over_knobs(effect(cached_token_ratio + 1)) < 1",
            "takes a quantity name first",
        ),
        (
            "max_over_knobs(effect(cached_token_ratio, ttft_p50_ms)) < 1",
            "1 to 1 arguments",
        ),
        (
            "max_over_knobs(ci_low(cached_token_ratio, ci = 1)) < 1",
            "strictly between 0 and 1",
        ),
        (
            "max_over_knobs(ci_low(cached_token_ratio, control)) < 1",
            "`ci = <level>`",
        ),
        ("noise_floor(cached_token_ratio) > 0", "2 to 3 arguments"),
        (
            "noise_floor(cached_token_ratio, treatment) > 0",
            "takes `control` second",
        ),
        (
            "max_over_knobs(abs(control)) < 1",
            "argument 1 is an expression",
        ),
        (
            "median(cached_token_ratio) < 1",
            "not a [measures] quantity",
        ),
    ] {
        let e = err(candidate(&with_predicate(p), "t1").0);
        assert!(e.contains(needle), "{p}: {e}");
    }
    assert!(candidate(
        &with_predicate("min_over_knobs(ci_high(cached_token_ratio, ci = 0.9)) < noise_floor(cached_token_ratio, control, ci = 0.9)"),
        "t1"
    )
    .0
    .is_ok());
}

/// Cites: HYP-14
#[test]
fn a_per_cell_predicate_over_many_cells_needs_at_or_an_aggregate() {
    // Four cells per slice: a bare per-cell comparison is ambiguous.
    let e = err(candidate(&with_predicate("effect(cached_token_ratio) < 0.1"), "t1").0);
    assert!(e.contains("ambiguous"), "{e}");
    assert!(
        candidate(
            &with_predicate("effect(cached_token_ratio) < 0.1 at all cells"),
            "t1"
        )
        .0
        .is_ok()
    );
    assert!(
        candidate(
            &with_predicate("max_over_knobs(effect(cached_token_ratio)) < 0.1"),
            "t1"
        )
        .0
        .is_ok()
    );
    // A select that fixes every pooled parameter is slice-level.
    assert!(candidate(
        &with_predicate("cached_token_ratio(knob = true, fast) - cached_token_ratio(control, knob = false, slow) < 0.1"),
        "t1"
    )
    .0
    .is_ok());
    // One cell per slice: nothing to disambiguate.
    let one = BASE
        .replace("knob = { kind = \"bool\" }\n", "")
        .replace("values = [\"fast\", \"slow\"]", "values = [\"fast\"]")
        .replace("config = { knob = false }", "config = { mode = \"fast\" }")
        .replace(
            "predicate = \"max_over_knobs(abs(effect(cached_token_ratio))) < noise_floor(cached_token_ratio, control)\"",
            "predicate = \"effect(cached_token_ratio) < 0.1\"",
        );
    assert!(candidate(&one, "t1").0.is_ok());
    // `at` bounds a range, and only a pooled one.
    let e = err(candidate(
        &with_predicate("effect(cached_token_ratio) < 0.1 at all knob <= 1"),
        "t1",
    )
    .0);
    assert!(e.contains("not a range"), "{e}");
    let e = err(candidate(
        &with_predicate("effect(cached_token_ratio) < 0.1 at all speed <= 1"),
        "t1",
    )
    .0);
    assert!(e.contains("not a parameter"), "{e}");
}
