//! HYP-4, HYP-25: a hypothesis directory made read-only still loads, lints and
//! is judged, and nothing outside `runs/` is ever written; a changed file aborts
//! with `hypothesis_changed`; only `verdict::write` writes, under `runs/`; no
//! option relaxes a hypothesis, and a `fail` on a frozen one is written like any
//! other verdict.

#![allow(clippy::unwrap_used, clippy::expect_used, clippy::panic)] // CON-19: tests are exempt

mod common;

use std::collections::BTreeMap;
use std::path::Path;

use acn_hyp::verdict::{V, verdict, write};
use acn_hyp::{HYPOTHESIS_CHANGED, Status};
use common::bundles::{Spec, alt, bundle, engine};
use common::{BASE, frozen_text, root};

/// Every file under `dir` but `runs/`, with its bytes.
fn snapshot(dir: &Path) -> BTreeMap<String, Vec<u8>> {
    let mut out = BTreeMap::new();
    for e in walk(dir) {
        let rel = e
            .strip_prefix(dir)
            .unwrap()
            .to_string_lossy()
            .replace('\\', "/");
        if !rel.starts_with("runs/") {
            out.insert(rel, std::fs::read(&e).unwrap());
        }
    }
    out
}

fn walk(dir: &Path) -> Vec<std::path::PathBuf> {
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

#[cfg(unix)]
fn set_mode(p: &Path, mode: u32) {
    use std::os::unix::fs::PermissionsExt as _;
    std::fs::set_permissions(p, std::fs::Permissions::from_mode(mode)).unwrap();
}

/// Cites: HYP-4
#[test]
fn a_read_only_hypothesis_directory_is_read_and_nothing_outside_runs_is_written() {
    let text = frozen_text(BASE);
    let dir = root(&[("hypotheses/t1.toml", &text)]);
    let r = dir.path();
    let file = r.join("hypotheses/t1.toml");
    #[cfg(unix)]
    {
        set_mode(&file, 0o444);
        set_mode(&r.join("hypotheses"), 0o555);
    }
    let before = snapshot(r);
    let h = acn_hyp::load_in(&file, r).unwrap();
    assert_eq!(h.status(), Status::Frozen);
    assert!(acn_hyp::lint::lint_in(&file, r).ok());
    let v = verdict(&h, bundles(&h, 0.3), engine()).unwrap();
    let path = write(&r.join("runs"), &v).unwrap();
    assert!(path.starts_with(r.join("runs/verdicts")));
    h.check_unchanged().unwrap();
    assert_eq!(snapshot(r), before, "only runs/ changed");
    #[cfg(unix)]
    {
        set_mode(&r.join("hypotheses"), 0o755);
        set_mode(&file, 0o644);
    }
}

/// Cites: HYP-4
#[test]
fn a_changed_file_aborts_with_hypothesis_changed() {
    let dir = tempfile::tempdir().unwrap();
    let file = dir.path().join("t1.toml");
    std::fs::write(&file, BASE).unwrap();
    let h = acn_hyp::load_in(&file, dir.path()).unwrap();
    h.check_unchanged().unwrap();
    // A comment is a change: the hash is of the bytes (HYP-5).
    std::fs::write(&file, format!("{BASE}# edited\n")).unwrap();
    let e = h.check_unchanged().unwrap_err().to_string();
    assert!(e.contains(HYPOTHESIS_CHANGED), "{e}");
    std::fs::write(&file, BASE).unwrap();
    h.check_unchanged().unwrap();
    std::fs::remove_file(&file).unwrap();
    let e = h.check_unchanged().unwrap_err().to_string();
    assert!(
        e.contains(HYPOTHESIS_CHANGED) && e.contains("can no longer be read"),
        "{e}"
    );
}

/// Cites: HYP-4
#[test]
fn acn_hyp_writes_only_verdicts_and_only_under_runs() {
    // Every call in the crate's source that can create, change or remove a file.
    let src = Path::new(env!("CARGO_MANIFEST_DIR")).join("src");
    let writers = [
        "fs::write",
        "OpenOptions",
        "File::create",
        "create_dir",
        "remove_file",
        "remove_dir",
        "rename",
        "hard_link",
        "set_permissions",
    ];
    for f in walk(&src) {
        let text = std::fs::read_to_string(&f).unwrap();
        let name = f.file_name().unwrap().to_string_lossy().into_owned();
        for w in writers {
            if text.contains(w) {
                assert_eq!(name, "verdict.rs", "{w} in {}", f.display());
            }
        }
    }
    let h = common::candidate(BASE, "t1").0.unwrap();
    let v = verdict(
        &h,
        bundles(&h, 0.3)
            .into_iter()
            .map(|mut b| {
                b.manifest.params.insert("replicates".into(), "20".into());
                b
            })
            .collect(),
        engine(),
    );
    // BASE runs four replicates: twenty-replicate bundles are judged on 0..4.
    let v = v.unwrap();
    let tmp = tempfile::tempdir().unwrap();
    for bad in ["out", "runs/verdicts", "run"] {
        let target = tmp.path().join(bad);
        assert!(
            write(&target, &v)
                .unwrap_err()
                .to_string()
                .contains("HYP-4"),
            "{bad}"
        );
    }
    assert_eq!(walk(tmp.path()).len(), 0, "nothing written");
}

/// Cites: HYP-25
#[test]
fn a_fail_on_a_frozen_hypothesis_is_written_like_any_verdict() {
    let text = frozen_text(BASE);
    let dir = root(&[("hypotheses/t1.toml", &text)]);
    let h = acn_hyp::load_in(&dir.path().join("hypotheses/t1.toml"), dir.path()).unwrap();
    assert_eq!(h.status(), Status::Frozen);
    let pass = verdict(&h, bundles(&h, 0.3), engine()).unwrap();
    // No effect clears the noise floor: the hypothesis is refuted.
    let fail = verdict(&h, bundles(&h, 0.0), engine()).unwrap();
    assert_eq!((pass.verdict, fail.verdict), (V::Pass, V::Fail));
    let runs = dir.path().join("runs");
    for v in [&pass, &fail] {
        let p = write(&runs, v).unwrap();
        let j: serde_json::Value =
            serde_json::from_str(&std::fs::read_to_string(p).unwrap()).unwrap();
        assert_eq!(j["verdict"], v.verdict.as_str());
        assert_eq!(j["hypothesis"]["status"], "frozen");
    }
    // The same shape for both: a fail is a result, not a special case.
    let keys = |v: &acn_hyp::verdict::Verdict| -> Vec<String> {
        let j: serde_json::Value = serde_json::from_str(&v.text()).unwrap();
        j.as_object().unwrap().keys().cloned().collect()
    };
    assert_eq!(keys(&pass), keys(&fail));
}

/// Cites: HYP-25
#[test]
fn no_command_offers_to_relax_rewrite_or_rescope_a_hypothesis() {
    // The `acn hyp` subcommands and their arguments, as the CLI declares them.
    let main = std::fs::read_to_string(
        Path::new(env!("CARGO_MANIFEST_DIR")).join("../acn-cli/src/main.rs"),
    )
    .unwrap();
    let start = main.find("enum HypCmd").unwrap();
    let end = start + main[start..].find("\n}\n").unwrap();
    let hyp = main[start..end].to_lowercase();
    for word in [
        "relax",
        "override",
        "force",
        "skip",
        "exclude",
        "ignore",
        "rescope",
        "re-scope",
        "rewrite",
        "tolerance",
        "threshold",
        "allow",
        "accept",
    ] {
        assert!(!hyp.contains(word), "`acn hyp` declares `{word}`");
    }
    // The verdict takes the file, the bundles and the engine; nothing else can
    // change what it decides.
    let f: fn(
        &acn_hyp::Hypothesis,
        Vec<acn_hyp::read::BundleData>,
        acn_trace::identity::Digest,
    ) -> Result<acn_hyp::verdict::Verdict, acn_hyp::verdict::VerdictError> = verdict;
    let _ = f;
}

/// Cites: HYP-4
#[test]
fn ci_jobs_hold_no_write_access_to_the_repository() {
    let dir = Path::new(env!("CARGO_MANIFEST_DIR")).join("../../.github/workflows");
    let mut seen = 0;
    for f in walk(&dir) {
        let text = std::fs::read_to_string(&f).unwrap();
        let name = f.display().to_string();
        assert!(
            text.contains("contents: read"),
            "{name}: contents must be read-only"
        );
        for w in ["contents: write", "write-all", "pull-requests: write"] {
            assert!(!text.contains(w), "{name}: `{w}`");
        }
        seen += 1;
    }
    assert!(seen >= 2, "the CI and pr-check workflows");
}
