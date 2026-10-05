//! HYP-4, HYP-25: a hypothesis directory made writable is never written, and
//! one made read-only is still read; a changed file aborts with
//! `hypothesis_changed` and nothing is written; verdicts go to the workspace's
//! own `runs/` and nowhere else (that `verdict::write` is the crate's one writer
//! is checked structurally by xtask's `workspace.rs`); and nothing outside the
//! hashed bytes can
//! change what a frozen hypothesis decides, whose `fail` is written like any
//! other verdict.

#![allow(clippy::unwrap_used, clippy::expect_used, clippy::panic)] // CON-19: tests are exempt

mod common;

use std::collections::BTreeMap;
use std::path::{Path, PathBuf};

use acn_hyp::verdict::{V, VerdictError, judge_and_write, verdict, write};
use acn_hyp::{HYPOTHESIS_CHANGED, Status};
use common::bundles::{Spec, alt, bundle, engine};
use common::{BASE, frozen_text, root};

/// Every entry under `dir` but `runs/`: a file with its bytes, a directory as
/// `None`.
fn snapshot(dir: &Path) -> BTreeMap<String, Option<Vec<u8>>> {
    let mut out = BTreeMap::new();
    let mut stack = vec![dir.to_path_buf()];
    while let Some(d) = stack.pop() {
        for e in std::fs::read_dir(&d).unwrap() {
            let p = e.unwrap().path();
            let rel = p
                .strip_prefix(dir)
                .unwrap()
                .to_string_lossy()
                .replace('\\', "/");
            if rel == "runs" || rel.starts_with("runs/") {
                continue;
            }
            if p.is_dir() {
                out.insert(rel, None);
                stack.push(p);
            } else {
                out.insert(rel, Some(std::fs::read(&p).unwrap()));
            }
        }
    }
    out
}

/// The six bundles of BASE's slice, 20 replicates, treatment = control + `e`.
fn bundles(h: &acn_hyp::Hypothesis, e: f64) -> Vec<acn_hyp::read::BundleData> {
    let base = alt(20, 0.40, 0.46);
    let mut out = Vec::new();
    for m in ["fast", "slow"] {
        out.push(bundle(
            h,
            &Spec::new(
                &format!("{e}-c-{m}"),
                &[("knob", "false"), ("mode", m)],
                "control",
                base.clone(),
            ),
        ));
    }
    for k in ["false", "true"] {
        for m in ["fast", "slow"] {
            let r = base
                .iter()
                .map(|x| x.map(|x| x + if k == "true" { e } else { 0.0 }))
                .collect();
            out.push(bundle(
                h,
                &Spec::new(
                    &format!("{e}-t-{k}-{m}"),
                    &[("knob", k), ("mode", m)],
                    "treatment",
                    r,
                ),
            ));
        }
    }
    out
}

/// Load, lint, judge and write a verdict for the frozen `hypotheses/t1.toml`.
fn use_it(r: &Path) -> PathBuf {
    let file = r.join("hypotheses/t1.toml");
    let h = acn_hyp::load_in(&file, r).unwrap();
    assert_eq!(h.status(), Status::Frozen);
    assert!(acn_hyp::lint::lint_in(&file, r).ok());
    let (_, path) = judge_and_write(&h, bundles(&h, 0.3), engine(), &r.join("runs")).unwrap();
    h.check_unchanged().unwrap();
    path
}

/// Cites: HYP-4
#[test]
fn a_hypothesis_directory_made_writable_is_never_written() {
    let dir = root(&[("hypotheses/t1.toml", &frozen_text(BASE))]);
    let r = dir.path();
    let before = snapshot(r);
    let path = use_it(r);
    assert!(path.starts_with(r.join("runs/verdicts")));
    assert_eq!(
        snapshot(r),
        before,
        "nothing outside runs/ was created, changed or removed"
    );
}

/// Restores a path's permissions however the test ends.
#[cfg(unix)]
struct Mode(PathBuf, u32);

#[cfg(unix)]
impl Drop for Mode {
    fn drop(&mut self) {
        use std::os::unix::fs::PermissionsExt as _;
        let _ = std::fs::set_permissions(&self.0, std::fs::Permissions::from_mode(self.1));
    }
}

#[cfg(unix)]
fn read_only(p: &Path, mode: u32, restore: u32) -> Mode {
    use std::os::unix::fs::PermissionsExt as _;
    std::fs::set_permissions(p, std::fs::Permissions::from_mode(mode)).unwrap();
    Mode(p.to_path_buf(), restore)
}

/// Cites: HYP-4
#[test]
fn a_hypothesis_directory_made_read_only_is_still_read() {
    let dir = root(&[("hypotheses/t1.toml", &frozen_text(BASE))]);
    let r = dir.path();
    #[cfg(unix)]
    let _guards = (
        read_only(&r.join("hypotheses/t1.toml"), 0o444, 0o644),
        read_only(&r.join("hypotheses"), 0o555, 0o755),
    );
    let before = snapshot(r);
    use_it(r);
    assert_eq!(snapshot(r), before);
}

/// Cites: HYP-4
#[test]
fn a_file_changed_after_it_was_loaded_aborts_and_nothing_is_written() {
    let dir = root(&[("hypotheses/t1.toml", &frozen_text(BASE))]);
    let r = dir.path();
    let file = r.join("hypotheses/t1.toml");
    let h = acn_hyp::load_in(&file, r).unwrap();
    let b = bundles(&h, 0.3);
    h.check_unchanged().unwrap();
    // A comment is a change: the hash is of the bytes (HYP-5).
    std::fs::write(&file, format!("{}# edited\n", frozen_text(BASE))).unwrap();
    let e = judge_and_write(&h, b, engine(), &r.join("runs")).unwrap_err();
    match &e {
        VerdictError::HypothesisChanged(c) => {
            assert_eq!(c.was, h.hash());
            assert!(c.now.is_some() && c.now != Some(h.hash()));
        }
        other => panic!("{other:?}"),
    }
    assert!(e.to_string().starts_with(HYPOTHESIS_CHANGED), "{e}");
    assert!(!r.join("runs").exists(), "nothing written");
    // Restored, it is the same file again; removed, it cannot be read.
    std::fs::write(&file, frozen_text(BASE)).unwrap();
    h.check_unchanged().unwrap();
    std::fs::remove_file(&file).unwrap();
    let c = h.check_unchanged().unwrap_err();
    assert!(
        c.now.is_none() && c.to_string().contains("can no longer be read"),
        "{c}"
    );
}

/// Cites: HYP-4
#[test]
fn verdicts_go_to_the_workspaces_own_runs_and_nowhere_else() {
    let dir = root(&[("hypotheses/t1.toml", &frozen_text(BASE))]);
    let r = dir.path();
    let h = acn_hyp::load_in(&r.join("hypotheses/t1.toml"), r).unwrap();
    let v = verdict(&h, bundles(&h, 0.3), engine()).unwrap();
    // A `runs` that is a link to somewhere else is refused.
    #[cfg(unix)]
    {
        let elsewhere = tempfile::tempdir().unwrap();
        std::os::unix::fs::symlink(elsewhere.path(), r.join("runs")).unwrap();
        assert!(
            write(&r.join("runs"), &v)
                .unwrap_err()
                .to_string()
                .contains("HYP-4")
        );
        assert_eq!(std::fs::read_dir(elsewhere.path()).unwrap().count(), 0);
        std::fs::remove_file(r.join("runs")).unwrap();
    }
    for bad in [
        "hypotheses/runs",
        "scenarios/runs",
        "docs/runs",
        "out",
        "runs/verdicts",
    ] {
        std::fs::create_dir_all(r.join(bad).parent().unwrap()).unwrap();
        let e = write(&r.join(bad), &v).unwrap_err().to_string();
        assert!(e.contains("HYP-4"), "{bad}: {e}");
        assert!(!r.join(bad).join("verdicts").exists(), "{bad}");
    }
    let path = write(&r.join("runs"), &v).unwrap();
    assert!(path.starts_with(r.join("runs/verdicts")));
}

/// Cites: HYP-25
#[test]
fn a_fail_on_a_frozen_hypothesis_is_written_like_any_verdict() {
    let dir = root(&[("hypotheses/t1.toml", &frozen_text(BASE))]);
    let h = acn_hyp::load_in(&dir.path().join("hypotheses/t1.toml"), dir.path()).unwrap();
    assert_eq!(h.status(), Status::Frozen);
    let runs = dir.path().join("runs");
    let (pass, _) = judge_and_write(&h, bundles(&h, 0.3), engine(), &runs).unwrap();
    // No effect clears the noise floor: the hypothesis is refuted.
    let (fail, path) = judge_and_write(&h, bundles(&h, 0.0), engine(), &runs).unwrap();
    assert_eq!((pass.verdict, fail.verdict), (V::Pass, V::Fail));
    let j: serde_json::Value =
        serde_json::from_str(&std::fs::read_to_string(path).unwrap()).unwrap();
    assert_eq!(
        (&j["verdict"], &j["hypothesis"]["status"]),
        (&"fail".into(), &"frozen".into())
    );
    // The same shape for both: a fail is a result, not a special case.
    let keys = |v: &acn_hyp::verdict::Verdict| -> Vec<String> {
        let j: serde_json::Value = serde_json::from_str(&v.text()).unwrap();
        j.as_object().unwrap().keys().cloned().collect()
    };
    assert_eq!(keys(&pass), keys(&fail));
}

/// Cites: HYP-25
#[test]
fn nothing_but_the_hashed_bytes_decides_a_verdict() {
    // The verdict takes the hypothesis, the bundles and the engine, and nothing
    // else; and a hypothesis outside this crate is read through accessors only
    // (its fields are private), so no caller can relax its design, guard or
    // predicate between loading it and judging with it. The CLI half is pinned by
    // `acn-cli`'s `acn_hyp_offers_no_option_to_relax_a_hypothesis`.
    let f: fn(
        &acn_hyp::Hypothesis,
        Vec<acn_hyp::read::BundleData>,
        acn_trace::identity::Digest,
    ) -> Result<acn_hyp::verdict::Verdict, VerdictError> = verdict;
    let dir = root(&[("hypotheses/t1.toml", &frozen_text(BASE))]);
    let h = acn_hyp::load_in(&dir.path().join("hypotheses/t1.toml"), dir.path()).unwrap();
    // The same file judges the same set the same way, every time.
    let a = f(&h, bundles(&h, 0.0), engine()).unwrap();
    let b = f(&h.clone(), bundles(&h, 0.0), engine()).unwrap();
    assert_eq!(a.text(), b.text());
    assert_eq!(h.design().replicates, 20, "read, not written");
}
