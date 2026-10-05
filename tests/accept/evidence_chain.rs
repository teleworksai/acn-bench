//! SPEC 085 acceptance, LOOP-1 and LOOP-2: an L1 chain (loop report →
//! verdict → bundles), made by the real harness in `sim`, verifies from its
//! loop_id and from its verdict_id. Breaking any link fails it: a bundle file,
//! the recorded layer, the verdict's bytes. A verdict no loop report names, and
//! a binary of another build, fail too. The L2 and L3 links come with T11b and
//! T30, and the evidence pages (LOOP-30) with T07.

#![allow(clippy::unwrap_used, clippy::expect_used, clippy::panic)] // CON-19: tests are exempt

use std::path::Path;

use acn_accept::build;
use acn_cli::loop_exec::HarnessExecutor;
use acn_hyp::evidence::{self, Checked};
use acn_hyp::loop_run::{self, Args, Binary, Code, Completed};
use acn_trace::identity::Digest;

const HYP: &str = r#"[poc]
id = "zz"
title = "tool order and the cache"

[hypothesis]
statement = "A stable tool order moves the cached-token ratio."

[varies]
tool_order_stable = { kind = "bool" }

[measures]
primary = ["cached_token_ratio"]
secondary = ["ttft_p50_ms"]

[control]
description = "the shipped default"
config = { tool_order_stable = true }

[design]
search = "grid"
replicates = 4
twin_required = false

[falsifier]
predicate = "max_over_knobs(abs(effect(cached_token_ratio))) < 0.001"
inconclusive_if = "replicates < 4"

[expected]
outcome = "pass"
"#;

/// The executor `acn` ships (LOOP-15), with a test build identity.
struct Harness(HarnessExecutor);

impl Harness {
    fn bin(&self) -> Binary {
        Binary {
            engine_hash: self.0.engine_hash,
            build_hash: Digest::from_hex(&self.0.build.build_hash).unwrap(),
        }
    }
}

fn harness_of(tag: &str) -> Harness {
    Harness(HarnessExecutor {
        engine_hash: Digest::of(b"engine"),
        build: build(tag),
    })
}

fn harness() -> Harness {
    harness_of("accept")
}

/// A directory with the hypothesis and the smoke workload, and a loop run in
/// it with `budget`.
fn chain(dir: &Path, budget: u64) -> Completed {
    std::fs::write(dir.join("zz.toml"), HYP).unwrap();
    std::fs::copy(
        concat!(
            env!("CARGO_MANIFEST_DIR"),
            "/../../workloads/harness-smoke.toml"
        ),
        dir.join("w.toml"),
    )
    .unwrap();
    let h = acn_hyp::load_in(&dir.join("zz.toml"), dir).unwrap();
    let mut x = harness();
    let bin = x.bin();
    let args = Args {
        workloads: vec!["w.toml".into()],
        models: vec!["mock-auto".into()],
        budget,
    };
    loop_run::run(&h, &args, &dir.join("runs"), bin, &mut x.0).unwrap()
}

fn check(dir: &Path, id: &Digest) -> Checked {
    let mut x = harness();
    let bin = x.bin();
    evidence::verify(&dir.join("runs"), &id.to_hex(), bin, &mut x.0).unwrap()
}

fn codes(c: &Checked) -> Vec<Code> {
    c.findings.iter().map(|f| f.code).collect()
}

/// Cites: LOOP-2, LOOP-1, LOOP-14
#[test]
fn an_l1_chain_verifies_from_its_loop_and_from_its_verdict() {
    let dir = tempfile::tempdir().unwrap();
    let d = dir.path();
    let c = chain(d, 10);
    for id in [&c.loop_id, &c.verdict_id] {
        let k = check(d, id);
        assert!(k.ok(), "{:?}", k.findings);
        assert_eq!(k.loops, [c.loop_id.to_hex()]);
        assert_eq!((k.bundles, k.verdicts, k.regenerated.len()), (3, 1, 1));
    }
    // Two loops that end on one bundle set: the verdict names both chains.
    let other = {
        let mut x = harness();
        let bin = x.bin();
        let h = acn_hyp::load_in(&d.join("zz.toml"), d).unwrap();
        let args = Args {
            workloads: vec!["w.toml".into()],
            models: vec!["mock-auto".into()],
            budget: 20,
        };
        loop_run::run(&h, &args, &d.join("runs"), bin, &mut x.0).unwrap()
    };
    let k = check(d, &c.verdict_id);
    assert!(k.ok(), "{:?}", k.findings);
    let mut both = vec![c.loop_id.to_hex(), other.loop_id.to_hex()];
    both.sort();
    assert_eq!(k.loops, both);
}

/// A fresh directory with a completed loop of budget 10.
fn fresh() -> (tempfile::TempDir, Completed) {
    let dir = tempfile::tempdir().unwrap();
    let c = chain(dir.path(), 10);
    (dir, c)
}

/// Edit `report.json` of `c` as text.
fn edit_report(c: &Completed, f: impl Fn(String) -> String) {
    let text = std::fs::read_to_string(&c.report).unwrap();
    std::fs::write(&c.report, f(text)).unwrap();
}

/// Cites: LOOP-2, LOOP-1
#[test]
fn breaking_any_link_fails_the_chain_with_its_code() {
    use Code::{
        BundleInvalid, HypothesisChanged, LayerMismatch, NotRegenerable, NotRegenerated, Report,
        VerdictMismatch,
    };
    let fails = |break_it: &dyn Fn(&Path, &Completed), want: &[Code]| {
        let (dir, c) = fresh();
        break_it(dir.path(), &c);
        let k = check(dir.path(), &c.loop_id);
        assert!(!k.ok());
        assert_eq!(codes(&k), want, "{:?}", k.findings);
    };
    // A bundle's file: it does not verify, the verdict without it is another,
    // and the report does not regenerate.
    fails(
        &|d, c| {
            let first = c.run_ids[0].to_hex();
            std::fs::write(d.join("runs").join(first).join("spans.parquet"), b"x").unwrap();
        },
        &[BundleInvalid, VerdictMismatch, NotRegenerated],
    );
    // The digest a report records for a bundle that still verifies.
    fails(
        &|_, c| {
            let first = c.run_ids[0].to_hex();
            edit_report(c, |t| {
                let at = t.find(&format!("\"run_id\":\"{first}\"")).unwrap();
                let d = t[..at].rfind("\"bundle_digest\":\"").unwrap() + 17;
                format!("{}{}{}", &t[..d], "0".repeat(64), &t[d + 64..])
            });
        },
        &[BundleInvalid, NotRegenerated],
    );
    // The layer a report records.
    fails(
        &|_, c| edit_report(c, |t| t.replace("\"layer\":\"L1\"", "\"layer\":\"L2\"")),
        &[LayerMismatch, NotRegenerated],
    );
    // The verdict's bytes, or the verdict itself.
    let vpath = |d: &Path, c: &Completed| {
        d.join("runs/verdicts")
            .join(c.verdict_id.to_hex())
            .join("verdict.json")
    };
    fails(
        &|d, c| std::fs::write(vpath(d, c), "{}\n").unwrap(),
        &[VerdictMismatch, NotRegenerated],
    );
    fails(
        &|d, c| std::fs::remove_file(vpath(d, c)).unwrap(),
        &[VerdictMismatch, NotRegenerated],
    );
    // A field the walk needs, missing: the report is refused, nothing guessed.
    fails(
        &|_, c| edit_report(c, |t| t.replace("\"build_hash\"", "\"no_build_hash\"")),
        &[Report],
    );
    // A listed bundle with no run_id.
    fails(
        &|_, c| {
            let first = c.run_ids[0].to_hex();
            edit_report(c, |t| {
                t.replacen(&format!("\"run_id\":\"{first}\""), "\"run\":\"x\"", 1)
            });
        },
        // The regeneration refuses the malformed report too.
        &[Report, VerdictMismatch, Report],
    );
    // The hypothesis edited: the walk ends there, once.
    fails(
        &|d, _| {
            let p = d.join("zz.toml");
            let t = std::fs::read_to_string(&p).unwrap();
            std::fs::write(&p, format!("{t}# edited\n")).unwrap();
        },
        &[HypothesisChanged],
    );
    // Another binary cannot regenerate or recompute (CON-31).
    let (dir, c) = fresh();
    let mut other = harness_of("other");
    let bin = other.bin();
    let k = evidence::verify(
        &dir.path().join("runs"),
        &c.loop_id.to_hex(),
        bin,
        &mut other.0,
    )
    .unwrap();
    assert_eq!(codes(&k), [NotRegenerable], "{:?}", k.findings);
}

/// Cites: LOOP-2, HYP-4
#[test]
fn a_chain_is_read_from_runs_itself_and_never_through_a_link() {
    // A loop directory replaced by a link to the same loop in another tree.
    let (a, c) = fresh();
    let (b, _) = fresh();
    let loop_dir = a.path().join("runs/loop").join(c.loop_id.to_hex());
    std::fs::remove_dir_all(&loop_dir).unwrap();
    #[cfg(unix)]
    {
        std::os::unix::fs::symlink(
            b.path().join("runs/loop").join(c.loop_id.to_hex()),
            &loop_dir,
        )
        .unwrap();
        let k = check(a.path(), &c.loop_id);
        assert_eq!(codes(&k), [Code::Report], "{:?}", k.findings);
        assert!(
            !b.path().join("runs/regen").exists(),
            "nothing written there"
        );
        // `--from-report` through the link is refused the same way.
        let mut x = harness();
        let bin = x.bin();
        let e = loop_run::regenerate(&loop_dir.join("report.json"), bin, &mut x.0).unwrap_err();
        assert_eq!(e.code, Code::Report, "{e}");
    }
}

/// Cites: LOOP-2
#[test]
fn a_report_that_cannot_be_read_is_named_not_skipped() {
    let (dir, c) = fresh();
    edit_report(&c, |_| "not json".into());
    // By verdict_id: the unreadable report might have named it.
    let k = check(dir.path(), &c.verdict_id);
    assert_eq!(codes(&k), [Code::Report], "{:?}", k.findings);
    assert!(k.loops.is_empty());
    // By loop_id.
    let k = check(dir.path(), &c.loop_id);
    assert_eq!(codes(&k), [Code::Report], "{:?}", k.findings);
    // A staging directory a crashed writer left is not a report.
    let (dir, c) = fresh();
    std::fs::create_dir(dir.path().join("runs/loop/.x.partial.0")).unwrap();
    assert!(check(dir.path(), &c.verdict_id).ok());
}

/// Cites: LOOP-2
#[test]
fn a_verdict_no_loop_report_names_cannot_be_verified() {
    let dir = tempfile::tempdir().unwrap();
    let d = dir.path();
    let c = chain(d, 10);
    // `acn hyp verdict` over two of the loop's bundles: a verdict of its own,
    // with no recorded inputs to regenerate from.
    let h = acn_hyp::load_in(&d.join("zz.toml"), d).unwrap();
    let set = c.run_ids[..2]
        .iter()
        .map(|id| acn_hyp::read::read(&d.join("runs").join(id.to_hex())).unwrap())
        .collect();
    let v = acn_hyp::verdict::verdict(&h, set, Digest::of(b"engine")).unwrap();
    acn_hyp::verdict::write(&d.join("runs"), &v).unwrap();
    let k = check(d, &v.verdict_id);
    assert!(!k.ok());
    assert_eq!(codes(&k), [Code::NoLoopReport]);
    assert!(k.loops.is_empty());
    // Not an id at all.
    let mut x = harness();
    let bin = x.bin();
    for bad in ["nope", &"A".repeat(64), "../../etc"] {
        let e = evidence::verify(&d.join("runs"), bad, bin, &mut x.0).unwrap_err();
        assert_eq!(e.code, Code::BadId, "{bad}");
    }
}
