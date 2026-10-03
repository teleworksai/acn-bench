//! SPEC 080 acceptance, HYP-27: every frozen file lints clean; a predicate that
//! can never fire, or always fires, or reads no primary quantity, is rejected in
//! a frozen file; lint warns about point estimates; a broken candidate is
//! reported without failing the build.

#![allow(clippy::unwrap_used, clippy::expect_used, clippy::panic)] // CON-19: tests are exempt

use std::path::{Path, PathBuf};

use acn_hyp::Status;
use acn_hyp::lint::{lint, lint_hypothesis};

fn repo() -> PathBuf {
    Path::new(env!("CARGO_MANIFEST_DIR")).join("../..")
}

fn toml_files(dir: &Path) -> Vec<PathBuf> {
    let mut out: Vec<PathBuf> = std::fs::read_dir(dir)
        .unwrap()
        .map(|e| e.unwrap().path())
        .filter(|p| p.extension().is_some_and(|x| x == "toml"))
        .collect();
    out.sort();
    out
}

/// Cites: HYP-27
#[test]
fn every_frozen_file_lints_clean_and_candidates_are_only_reported() {
    let frozen = toml_files(&repo().join("hypotheses"));
    assert!(!frozen.is_empty());
    for f in &frozen {
        let r = lint(f);
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
        let r = lint(&f);
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

[falsifier]
predicate = "PREDICATE"

[expected]
outcome = "pass"
"#;

/// Load `predicate` in a file frozen under a temporary root.
fn frozen(predicate: &str) -> acn_hyp::Hypothesis {
    let text = BASE.replace("PREDICATE", predicate);
    let dir = tempfile::tempdir().unwrap();
    let r = dir.path();
    std::fs::create_dir_all(r.join("hypotheses")).unwrap();
    std::fs::create_dir_all(r.join("specs")).unwrap();
    std::fs::write(r.join("specs/README.md"), "100-x.md\n").unwrap();
    std::fs::write(r.join("hypotheses/t1.toml"), &text).unwrap();
    let record = serde_json::json!({
        "env_hash": "0".repeat(64), "engine_hash": "0".repeat(64),
        "files": [{ "path": "hypotheses/t1.toml", "blake3": blake3::hash(text.as_bytes()).to_hex().to_string() }],
    });
    std::fs::write(r.join("env-hash.json"), record.to_string()).unwrap();
    let h = acn_hyp::load(&r.join("hypotheses/t1.toml")).unwrap();
    assert_eq!(h.status, Status::Frozen);
    h
}

/// Cites: HYP-27
#[test]
fn a_frozen_falsifier_must_be_able_to_fire_and_to_fail_to_fire() {
    let clean = lint_hypothesis(&frozen(
        "max_over_knobs(abs(effect(cached_token_ratio))) < noise_floor(cached_token_ratio, control)",
    ));
    assert!(clean.ok(), "{:?}", clean.errors);
    for (p, needle) in [
        ("max_over_knobs(cached_token_ratio) < 0", "can never fire"),
        ("max_over_knobs(cached_token_ratio) >= 0", "always fires"),
        ("max_over_knobs(ttft_p50_ms) > 1", "no primary quantity"),
    ] {
        let r = lint_hypothesis(&frozen(p));
        assert!(!r.ok(), "{p}");
        assert!(
            r.errors.iter().any(|e| e.contains(needle)),
            "{p}: {:?}",
            r.errors
        );
    }
}

/// Cites: HYP-27
#[test]
fn lint_warns_about_point_estimates() {
    let r = lint_hypothesis(&frozen("effect(cached_token_ratio) < 0.1 at all cells"));
    assert!(r.ok());
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
    let r = lint_hypothesis(&frozen("ci_high(cached_token_ratio) < 0.1 at all cells"));
    assert!(r.warnings.is_empty(), "an interval bound: {:?}", r.warnings);
}

/// Cites: HYP-27, HYP-1
#[test]
fn a_broken_file_is_reported_with_its_reason() {
    let dir = tempfile::tempdir().unwrap();
    let p = dir.path().join("t1.toml");
    std::fs::write(
        &p,
        BASE.replace("PREDICATE", "max_over_knobs(cached_token_ratio <"),
    )
    .unwrap();
    let r = lint(&p);
    assert!(!r.ok());
    assert!(
        r.errors[0].contains("falsifier.predicate"),
        "{:?}",
        r.errors
    );
}
