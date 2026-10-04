//! HYP-26: the freeze PR's shape is what `cargo xtask env-hash --check` and
//! `cargo xtask pr-check` enforce. A changed hypothesis file fails the record
//! check and loads as a candidate; a file added under `hypotheses/` must be
//! recorded, load (its POC spec named, its status consistent), lint clean and,
//! for `real-api`, carry its pins.

#![allow(clippy::unwrap_used, clippy::expect_used, clippy::panic)] // CON-19: tests are exempt

mod common;

use acn_hyp::Status;
use common::{BASE, frozen_text, record, root, with_predicate};
use xtask::env_hash::{self, Mode};
use xtask::pr_check::{self, Changes};

const FILE: &str = "hypotheses/t1.toml";

/// The HYP-26 findings of a PR that changes `FILE`, labelled `env-change`.
fn freeze_findings(r: &std::path::Path) -> Vec<String> {
    let report =
        pr_check::run(r, Changes::List(vec![FILE.into()]), &["env-change".into()]).unwrap();
    report
        .violations
        .iter()
        .filter(|v| v.rule == "HYP-26")
        .map(|v| v.message.clone())
        .collect()
}

/// Cites: HYP-26, HYP-3, HYP-5
#[test]
fn env_hash_check_fails_on_a_changed_hypothesis_file_and_the_file_is_no_longer_frozen() {
    let dir = root(&[(FILE, &frozen_text(BASE))]);
    let r = dir.path();
    assert!(env_hash::run(r, Mode::Check).unwrap().ok);
    let h = acn_hyp::load_in(&r.join(FILE), r).unwrap();
    assert_eq!(h.status(), Status::Frozen);
    // One byte of a comment is a change (HYP-5).
    std::fs::write(
        r.join(FILE),
        format!("{}# reformatted\n", frozen_text(BASE)),
    )
    .unwrap();
    let report = env_hash::run(r, Mode::Check).unwrap();
    assert!(!report.ok, "the freeze check");
    assert_eq!(report.diff.unwrap().changed, [FILE]);
    let h = acn_hyp::load_in(&r.join(FILE), r).unwrap();
    assert_eq!(
        h.status(),
        Status::Candidate,
        "a locally edited copy is a candidate"
    );
    // Recording the change is the Class C PR's job; then it is frozen again.
    record(r);
    assert!(env_hash::run(r, Mode::Check).unwrap().ok);
    assert_eq!(
        acn_hyp::load_in(&r.join(FILE), r).unwrap().status(),
        Status::Frozen
    );
}

/// Cites: HYP-26, HYP-2, HYP-3, HYP-27
#[test]
fn pr_check_enforces_the_shape_of_a_freeze() {
    // A complete freeze: recorded, spec named, lint clean.
    let dir = root(&[(FILE, &frozen_text(BASE))]);
    assert_eq!(freeze_findings(dir.path()), Vec::<String>::new());
    // Moved into place but not recorded: still a candidate.
    let dir = root(&[]);
    std::fs::write(dir.path().join(FILE), frozen_text(BASE)).unwrap();
    let f = freeze_findings(dir.path());
    assert!(
        f[0].contains("loads as a candidate") && f[0].contains("env-hash --write"),
        "{f:?}"
    );
    // [poc].status left saying candidate: the load refuses the mismatch.
    let text = frozen_text(BASE).replace(
        "title = \"a test\"",
        "title = \"a test\"\nstatus = \"candidate\"",
    );
    let f = freeze_findings(root(&[(FILE, &text)]).path());
    assert!(
        f[0].contains("does not load") && f[0].contains("status"),
        "{f:?}"
    );
    // No POC spec.
    let text = frozen_text(BASE).replace("spec = \"specs/100-x.md\"\n", "");
    let f = freeze_findings(root(&[(FILE, &text)]).path());
    assert!(
        f[0].contains("does not load") && f[0].contains("spec"),
        "{f:?}"
    );
    // A falsifier that can never fire.
    let text = frozen_text(&with_predicate(
        "max_over_knobs(abs(effect(cached_token_ratio))) < 0",
    ));
    let f = freeze_findings(root(&[(FILE, &text)]).path());
    assert!(
        f[0].contains("lint:") && f[0].contains("can never fire"),
        "{f:?}"
    );
    // real-api without pins, then with them.
    let real = frozen_text(BASE).replace(
        "twin_required = false",
        "twin_required = false\nbackends = [\"real-api\"]",
    );
    let f = freeze_findings(root(&[(FILE, &real)]).path());
    assert!(f[0].contains("[design].pins"), "{f:?}");
    let pinned = real.replace(
        "backends = [\"real-api\"]",
        &format!(
            "backends = [\"real-api\"]\npins = {{ scenario = [\"{s}\"], workload = [\"{s}\"], models = {{}} }}",
            s = acn_trace::identity::Digest::of(b"x").to_hex()
        ),
    );
    assert_eq!(
        freeze_findings(root(&[(FILE, &pinned)]).path()),
        Vec::<String>::new()
    );
    // A file the PR removes is the frozen-set change CON-7 covers, not a freeze.
    let dir = root(&[]);
    assert_eq!(freeze_findings(dir.path()), Vec::<String>::new());
    // Candidates elsewhere are not frozen and not checked here (CON-23).
    let report = pr_check::run(
        root(&[("lab/hypotheses/t1.toml", BASE)]).path(),
        Changes::List(vec!["lab/hypotheses/t1.toml".into()]),
        &[],
    )
    .unwrap();
    assert!(report.violations.iter().all(|v| v.rule != "HYP-26"));
}
