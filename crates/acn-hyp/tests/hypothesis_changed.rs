//! LOOP-13, HYP-4: the loop treats the hypothesis and workload files as
//! read-only input; a file that changes between batches aborts the loop with
//! `hypothesis_changed` or `input_changed`, and an abort writes neither a report
//! nor a final verdict.

#![allow(clippy::unwrap_used, clippy::expect_used, clippy::panic)] // CON-19: tests are exempt

mod common;

use acn_hyp::loop_run::Code;
use common::exec::{Exec, TWO, args, bundles, dir_with, run_loop};

/// Run TWO with a hook that edits `file` (relative to the directory) after
/// bundle `at` is made, and return the abort.
fn edited_after(file: &'static str, at: usize) -> (Code, tempfile::TempDir) {
    let dir = dir_with(TWO);
    let d = dir.path().to_path_buf();
    let mut ex = Exec::new(&d).hook(move |n, _, out| {
        if n == at {
            let p = d.join(file);
            let text = std::fs::read_to_string(&p).unwrap();
            std::fs::write(&p, format!("{text}# edited\n")).unwrap();
        }
        Ok(out)
    });
    let e = run_loop(dir.path(), args(10), &mut ex).unwrap_err();
    assert!(!dir.path().join("runs/loop").exists(), "{e}: no report");
    assert!(
        !dir.path().join("runs/verdicts").exists(),
        "{e}: no verdict"
    );
    (e.code, dir)
}

/// Cites: LOOP-13, HYP-4
#[test]
fn a_hypothesis_edited_between_batches_aborts_the_loop() {
    // After the first batch (two bundles): caught when the next batch starts.
    let (c, dir) = edited_after("zz.toml", 1);
    assert_eq!(c, Code::HypothesisChanged);
    assert_eq!(c.as_str(), acn_hyp::HYPOTHESIS_CHANGED);
    assert_eq!(bundles(dir.path()).len(), 2, "the batch made stays");
}

/// Cites: LOOP-13, HYP-4
#[test]
fn a_hypothesis_edited_during_the_last_batch_aborts_before_the_verdict() {
    // The third bundle is the last batch's: caught before the final verdict.
    let (c, dir) = edited_after("zz.toml", 2);
    assert_eq!(c, Code::HypothesisChanged);
    assert_eq!(bundles(dir.path()).len(), 3);
}

/// Cites: LOOP-13
#[test]
fn a_workload_edited_between_batches_aborts_the_loop() {
    let (c, _dir) = edited_after("w.toml", 0);
    assert_eq!(c, Code::InputChanged);
}

/// Cites: LOOP-13, HYP-4
#[test]
fn a_hypothesis_edited_after_loading_aborts_before_anything_runs() {
    let dir = dir_with(TWO);
    let d = dir.path();
    let h = acn_hyp::load_in(&d.join("zz.toml"), d).unwrap();
    std::fs::write(d.join("zz.toml"), format!("{TWO}# edited\n")).unwrap();
    let mut ex = Exec::new(d);
    let bin = ex.bin();
    let e = acn_hyp::loop_run::run(
        &h,
        &common::exec::args_in(d, args(10)),
        &d.join("runs"),
        bin,
        &mut ex,
    )
    .unwrap_err();
    assert_eq!(e.code, Code::HypothesisChanged, "{e}");
    assert!(ex.requests.is_empty());
}
