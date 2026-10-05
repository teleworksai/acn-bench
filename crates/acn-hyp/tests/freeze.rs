//! HYP-26: the freeze check is that `env-hash.json` records the frozen set as it
//! stands (`cargo xtask env-hash --check` compares the whole record, CON-28). A
//! changed hypothesis file breaks the record and loads as a candidate until a
//! Class C PR records it again. The rest of the freeze PR's shape is
//! `cargo xtask pr-check`'s, tested in `crates/xtask/tests/freeze.rs`.

#![allow(clippy::unwrap_used, clippy::expect_used, clippy::panic)] // CON-19: tests are exempt

mod common;

use acn_hyp::Status;
use acn_trace::env::{compute, read_record};
use common::{BASE, frozen_text, record, root};

const FILE: &str = "hypotheses/t1.toml";

/// What `env-hash --check` decides: the record is the frozen set as it stands.
fn record_holds(r: &std::path::Path) -> bool {
    read_record(r).unwrap() == Some(compute(r).unwrap())
}

/// Cites: HYP-26, HYP-3, HYP-5
#[test]
fn a_changed_hypothesis_file_breaks_the_record_and_is_no_longer_frozen() {
    let dir = root(&[(FILE, &frozen_text(BASE))]);
    let r = dir.path();
    assert!(record_holds(r));
    assert_eq!(
        acn_hyp::load_in(&r.join(FILE), r).unwrap().status(),
        Status::Frozen
    );
    // One byte of a comment is a change (HYP-5).
    std::fs::write(
        r.join(FILE),
        format!("{}# reformatted\n", frozen_text(BASE)),
    )
    .unwrap();
    assert!(!record_holds(r), "the freeze check fails");
    let changed: Vec<String> = compute(r)
        .unwrap()
        .files
        .iter()
        .filter(|f| {
            read_record(r)
                .unwrap()
                .unwrap()
                .files
                .iter()
                .any(|g| g.path == f.path && g.blake3 != f.blake3)
        })
        .map(|f| f.path.clone())
        .collect();
    assert_eq!(changed, [FILE]);
    let h = acn_hyp::load_in(&r.join(FILE), r).unwrap();
    assert_eq!(
        h.status(),
        Status::Candidate,
        "a locally edited copy is a candidate"
    );
    // Recording the change is the Class C PR's job; then it is frozen again.
    record(r);
    assert!(record_holds(r));
    assert_eq!(
        acn_hyp::load_in(&r.join(FILE), r).unwrap().status(),
        Status::Frozen
    );
}

/// Cites: HYP-26, HYP-1
#[test]
fn a_file_under_hypotheses_that_is_not_toml_never_loads_as_frozen() {
    let dir = root(&[("hypotheses/p5", &frozen_text(BASE))]);
    let e = acn_hyp::load_in(&dir.path().join("hypotheses/p5"), dir.path()).unwrap_err();
    assert!(e.to_string().contains("<id>.toml"), "{e}");
}
