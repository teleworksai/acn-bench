//! SPEC 080 acceptance, HYP-27: every frozen file lints clean; in a frozen file a
//! predicate that can never fire, always fires, reads no primary quantity, or is
//! disarmed by its guard, is rejected; lint warns about point estimates; a
//! candidate's findings are reported without failing; a broken file is reported.

#![allow(clippy::unwrap_used, clippy::expect_used, clippy::panic)] // CON-19: tests are exempt

use std::path::{Path, PathBuf};

use acn_hyp::Status;
use acn_hyp::lint::{Report, lint, lint_hypothesis, lint_in};

fn repo() -> PathBuf {
    Path::new(env!("CARGO_MANIFEST_DIR")).join("../..")
}

/// Every file under `dir`, in subdirectories too, whatever its name: a file
/// that is not a hypothesis fails to load and so fails lint (HYP-27, HYP-1).
fn toml_files(dir: &Path) -> Vec<PathBuf> {
    let mut out = Vec::new();
    let mut stack = vec![dir.to_path_buf()];
    while let Some(d) = stack.pop() {
        for e in std::fs::read_dir(&d).unwrap() {
            let p = e.unwrap().path();
            if p.is_dir() {
                stack.push(p);
            } else {
                out.push(p);
            }
        }
    }
    out.sort();
    out
}

/// Cites: HYP-27
#[test]
fn every_frozen_file_lints_clean_and_candidates_are_only_reported() {
    let frozen = toml_files(&repo().join("hypotheses"));
    assert!(!frozen.is_empty());
    for f in &frozen {
        let r = lint_in(f, &repo());
        assert!(r.ok(), "{}: {:?}", f.display(), r.errors);
        assert_eq!(r.status, Some(Status::Frozen), "{}", f.display());
        assert!(
            r.fires.is_some() && r.holds.is_some(),
            "{}: a witness pair",
            f.display()
        );
    }
    // Advisory: whatever a candidate's lint says, it is reported, never a failure.
    for f in toml_files(&repo().join("lab/hypotheses")) {
        let r = lint_in(&f, &repo());
        eprintln!(
            "{}: ok={} errors={:?} warnings={:?}",
            f.display(),
            r.ok(),
            r.errors,
            r.warnings
        );
    }
}

const BASE: &str = r#"[poc]
id = "t1"
title = "a test"
spec = "specs/100-x.md"

[hypothesis]
statement = "s"

[varies]
knob = { kind = "bool" }
provider = { kind = "enum", values = ["a", "b"] }

[measures]
primary = ["cached_token_ratio"]
secondary = ["ttft_p50_ms"]

[control]
description = "defaults"
config = { knob = false }

[design]
search = "grid"
replicates = 20
twin_required = false
min_providers_for_verdict = 2

[falsifier]
predicate = "PREDICATE"
GUARD
[expected]
outcome = "pass"
"#;

const GOOD: &str =
    "max_over_knobs(abs(effect(cached_token_ratio))) < noise_floor(cached_token_ratio, control)";

/// A temporary workspace root with `text` frozen in it, and the root itself.
fn frozen_root(text: &str) -> (tempfile::TempDir, PathBuf) {
    let dir = tempfile::tempdir().unwrap();
    let r = dir.path();
    for d in acn_trace::env::FROZEN_SET {
        std::fs::create_dir_all(r.join(d)).unwrap();
    }
    std::fs::create_dir_all(r.join("specs")).unwrap();
    std::fs::write(r.join("specs/README.md"), "| 100-x.md |\n").unwrap();
    std::fs::write(r.join("hypotheses/t1.toml"), text).unwrap();
    let rec = acn_trace::env::compute(r).unwrap();
    std::fs::write(
        r.join("env-hash.json"),
        serde_json::to_string(&rec).unwrap(),
    )
    .unwrap();
    let path = r.join("hypotheses/t1.toml");
    (dir, path)
}

fn text(predicate: &str, guard: Option<&str>) -> String {
    let g = guard.map_or(String::new(), |g| format!("inconclusive_if = {g:?}\n"));
    BASE.replace("PREDICATE", predicate).replace("GUARD\n", &g)
}

/// Lint `predicate` (and `guard`) frozen.
fn frozen(predicate: &str, guard: Option<&str>) -> Report {
    let (dir, path) = frozen_root(&text(predicate, guard));
    let h = acn_hyp::load_in(&path, dir.path()).unwrap();
    assert_eq!(h.status(), Status::Frozen);
    lint_hypothesis(&h)
}

/// Lint `predicate` as a candidate.
fn candidate(predicate: &str) -> Report {
    let dir = tempfile::tempdir().unwrap();
    let p = dir.path().join("t1.toml");
    std::fs::write(&p, text(predicate, None)).unwrap();
    lint_in(&p, dir.path())
}

fn errs(r: &Report, needle: &str) -> bool {
    r.errors.iter().any(|e| e.contains(needle))
}

/// Cites: HYP-27, HYP-11, HYP-13
#[test]
fn a_frozen_falsifier_must_be_able_to_fire_and_to_fail_to_fire() {
    let clean = frozen(GOOD, Some("replicates < 20 or providers_reported < 2"));
    assert!(clean.ok(), "{:?}", clean.errors);
    for (p, needle) in [
        // `abs` is never negative, even with the signed probes.
        (
            "max_over_knobs(abs(cached_token_ratio)) < 0",
            "can never fire",
        ),
        (
            "max_over_knobs(abs(cached_token_ratio)) >= 0",
            "always fires",
        ),
        ("max_over_knobs(ttft_p50_ms) > 1", "no primary quantity"),
        // An interval bound is never above its other bound (ci_low ≤ ci_high).
        (
            "max_over_knobs(ci_low(cached_token_ratio)) > max_over_knobs(ci_high(cached_token_ratio))",
            "can never fire",
        ),
        // A zero noise floor is undefined, so this can never be true.
        (
            "noise_floor(cached_token_ratio, control) <= 0 and max_over_knobs(abs(cached_token_ratio)) >= 0",
            "can never fire",
        ),
        // Kleene: `true or undefined` is true, so this always fires.
        (
            "max_over_knobs(abs(cached_token_ratio)) >= 0 or noise_floor(cached_token_ratio, control) < 0",
            "always fires",
        ),
    ] {
        let r = frozen(p, None);
        assert!(errs(&r, needle), "{p}: {:?}", r.errors);
    }
    // More than ten terms: no search is made, and that is an error when frozen.
    let wide = (0..6)
        .map(|_| "ci_low(cached_token_ratio, ci = 0.5)".to_owned())
        .collect::<Vec<_>>()
        .join(" + ");
    let r = frozen(
        &format!(
            "max_over_knobs({wide} + ci_low(cached_token_ratio, ci = 0.6) + ci_low(cached_token_ratio, ci = 0.7) + ci_low(cached_token_ratio, ci = 0.8) + ci_low(cached_token_ratio, ci = 0.9) + ci_low(cached_token_ratio, ci = 0.99) + ci_low(cached_token_ratio, ci = 0.999)) > 1"
        ),
        None,
    );
    assert!(errs(&r, "more than 10 distinct terms"), "{:?}", r.errors);
}

/// Cites: HYP-27, HYP-24, HYP-10
#[test]
fn a_guard_that_disarms_the_falsifier_fails_lint() {
    for g in [
        "0 < 1",
        "replicates < 21",
        "providers_reported < 3",
        "replicates >= 20",
    ] {
        let r = frozen(GOOD, Some(g));
        assert!(errs(&r, "the guard is not false"), "{g}: {:?}", r.errors);
    }
    for g in [
        "replicates < 20",
        "providers_reported < 2",
        "replicates < 20 or providers_reported < 2",
    ] {
        assert!(frozen(GOOD, Some(g)).ok(), "{g}");
    }
}

/// Cites: HYP-27
#[test]
fn a_falsifier_about_a_negative_interval_bound_has_a_witness() {
    // An interval bound bounds an effect, so it is probed with negative values
    // too: "discipline never raises the cache ratio" is a falsifier, with no
    // warning about it (issue 10).
    let r = frozen("max_over_knobs(ci_high(cached_token_ratio)) < 0", None);
    assert!(r.ok(), "{:?}", r.errors);
    assert!(
        !r.warnings.iter().any(|w| w.contains("negative")),
        "{:?}",
        r.warnings
    );
    let fires = r.fires.as_ref().unwrap();
    assert!(fires.values().any(|v| *v < 0.0), "{fires:?}");
    // Its mirror is no longer said to fire always.
    let r = frozen("max_over_knobs(ci_high(cached_token_ratio)) >= 0", None);
    assert!(r.ok(), "{:?}", r.errors);
    // Only bounds are signed. A quantity is never negative, so a predicate
    // that needs a negative one can never fire, and is refused.
    let r = frozen("max_over_knobs(cached_token_ratio) < 0", None);
    assert!(errs(&r, "can never fire"), "{:?}", r.errors);
    // An effect is not a probe of its own: it is computed from its arms, so
    // it cannot be positive while the relative effect is negative.
    let r = frozen(
        "max_over_knobs(effect(cached_token_ratio)) > 0 and max_over_knobs(rel_effect(cached_token_ratio)) < 0",
        None,
    );
    assert!(errs(&r, "can never fire"), "{:?}", r.errors);
}

/// Cites: HYP-27
#[test]
fn a_candidates_findings_are_warnings() {
    let r = candidate("max_over_knobs(abs(cached_token_ratio)) < 0");
    assert!(r.ok(), "{:?}", r.errors);
    assert_eq!(r.status, Some(Status::Candidate));
    assert!(
        r.warnings.iter().any(|w| w.contains("can never fire")),
        "{:?}",
        r.warnings
    );
}

/// Cites: HYP-27
#[test]
fn lint_warns_about_point_estimates_and_only_then() {
    let r = frozen("effect(cached_token_ratio) < 0.1 at all cells", None);
    assert!(r.ok(), "{:?}", r.errors);
    assert!(
        r.warnings
            .iter()
            .any(|w| w.contains("neither an interval bound")),
        "{:?}",
        r.warnings
    );
    assert!(
        r.warnings
            .iter()
            .any(|w| w.contains("one noisy cell decides")),
        "{:?}",
        r.warnings
    );
    let r = frozen("ci_high(cached_token_ratio) < 0.1 at all cells", None);
    assert!(r.warnings.is_empty(), "an interval bound: {:?}", r.warnings);
}

/// Cites: HYP-27, HYP-1
#[test]
fn a_broken_file_is_reported_with_its_reason() {
    let dir = tempfile::tempdir().unwrap();
    let p = dir.path().join("t1.toml");
    std::fs::write(&p, text("max_over_knobs(cached_token_ratio <", None)).unwrap();
    let r = lint(&p);
    assert!(!r.ok());
    assert!(
        r.errors[0].contains("falsifier.predicate"),
        "{:?}",
        r.errors
    );
}

/// Cites: HYP-27
#[test]
fn the_witness_search_is_bounded_by_assignments_and_says_when_it_was_not_made() {
    // ADR-54: the search is made up to 2^20 assignments. An interval is two
    // slots of seven probe values, its low and its high bound; every other
    // term is one slot of four.
    const LEVELS: [&str; 5] = ["0.5", "0.6", "0.7", "0.8", "0.9"];
    let floors = |n: usize| -> Vec<String> {
        ["cached_token_ratio", "ttft_p50_ms"]
            .iter()
            .flat_map(|q| {
                LEVELS
                    .iter()
                    .map(move |l| format!("noise_floor({q}, control, ci = {l}) > 1"))
            })
            .take(n)
            .collect()
    };
    let intervals = |n: usize| -> Vec<String> {
        LEVELS
            .iter()
            .take(n)
            .map(|l| format!("max_over_knobs(ci_low(cached_token_ratio, ci = {l})) > 1"))
            .collect()
    };
    let unsearched = |parts: Vec<String>| {
        let r = candidate(&parts.join(" and "));
        let said = r
            .errors
            .iter()
            .chain(&r.warnings)
            .any(|m| m.contains("no witness search was made"));
        // Either the search was made and found both witnesses, or lint said
        // that it was not made; nothing else, a load error included.
        assert_eq!(said, r.fires.is_none(), "{:?} {:?}", r.errors, r.warnings);
        assert_eq!(said, r.holds.is_none(), "{:?} {:?}", r.errors, r.warnings);
        said
    };
    // Ten unsigned terms are 4^10 = 2^20 assignments exactly: searched.
    assert!(!unsearched(floors(10)));
    // Three intervals and one other term are 49^3 · 4 = 470 596: searched.
    assert!(!unsearched([intervals(3), floors(1)].concat()));
    // One more term makes 1 882 384, over the cap with eight slots only: not
    // searched, and lint says so rather than passing the file.
    assert!(unsearched([intervals(3), floors(2)].concat()));
    // One interval is searched with seven other terms and not with eight,
    // which is ten slots and was searched before bounds were signed.
    assert!(!unsearched([intervals(1), floors(7)].concat()));
    assert!(unsearched([intervals(1), floors(8)].concat()));
}
