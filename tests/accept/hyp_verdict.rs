//! SPEC 080 acceptance, HYP-20..24, HYP-15: real bundles from the harness, in
//! `sim` on the mock, read and verified (TRC-23), judged against a candidate
//! hypothesis twice with byte-identical `verdict.json`, labelled `exploratory`,
//! `mock-gated` and `sim-only`, and a set the hypothesis did not produce refused.
//! The falsifier compares with a fixed threshold: the mock is deterministic, so
//! sim control replicates agree exactly and a `noise_floor` would be zero, which
//! HYP-13 makes undefined (ADR-20).

#![allow(clippy::unwrap_used, clippy::expect_used, clippy::panic)] // CON-19: tests are exempt

use std::collections::BTreeMap;
use std::path::{Path, PathBuf};

use acn_harness::agent::Opts;
use acn_harness::run::{HypothesisArg, RunConfig, run};
use acn_harness::wire::Backend;
use acn_hyp::verdict::{V, verdict, write};
use acn_trace::identity::{BuildParts, Digest, Mode};

const HYP: &str = r#"[poc]
id = "zz"
title = "tool order and the cache"

[hypothesis]
statement = "A stable tool order moves the cached-token ratio by more than its noise floor."

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

fn cfg(dir: &Path, hyp: &Path, arm: &str, stable: &str) -> RunConfig {
    RunConfig {
        workload: PathBuf::from(concat!(
            env!("CARGO_MANIFEST_DIR"),
            "/../../workloads/harness-smoke.toml"
        )),
        backend: Backend::Mockllm,
        model: "mock-explicit".into(),
        mode: Mode::Sim,
        arm: arm.into(),
        replicates: 4,
        vary: BTreeMap::from([("tool_order_stable".to_owned(), stable.to_owned())]),
        opts: Opts::default(),
        hypothesis: HypothesisArg::File(hyp.to_path_buf()),
        runs_dir: dir.join("runs"),
        start_dir: dir.to_path_buf(),
        engine_hash: Digest::of(b"engine"),
        build: BuildParts {
            cargo_lock: Digest::of(b"lock"),
            rust_toolchain: Digest::of(b"toolchain"),
            cargo_config: Digest::of(b"config"),
            source_hash: Digest::of(b"source"),
            target: "accept",
            profile: "debug",
            features: "",
            rustflags: "",
        }
        .info()
        .unwrap(),
        profiles: None,
    }
}

/// Cites: HYP-20, HYP-21, HYP-23, HYP-15, HYP-28
#[test]
fn harness_bundles_are_read_judged_and_judged_the_same_way_twice() {
    let dir = tempfile::tempdir().unwrap();
    let hyp = dir.path().join("zz.toml");
    std::fs::write(&hyp, HYP).unwrap();
    let mut dirs = Vec::new();
    for (arm, stable) in [
        ("treatment", "false"),
        ("treatment", "true"),
        ("control", "true"),
    ] {
        dirs.push(run(&cfg(dir.path(), &hyp, arm, stable)).unwrap().dir);
    }
    let h = acn_hyp::load_in(&hyp, dir.path()).unwrap();
    let read = || {
        dirs.iter()
            .map(|d| acn_hyp::read::read(d).unwrap())
            .collect::<Vec<_>>()
    };
    let b = read();
    let replicates: usize = b
        .iter()
        .map(|x| {
            x.sessions
                .iter()
                .map(|s| s.replicate)
                .collect::<std::collections::BTreeSet<_>>()
                .len()
        })
        .sum();
    assert_eq!(
        replicates, 12,
        "4 replicates × 3 bundles (two sessions each, one per task)"
    );
    let v = verdict(&h, b, Digest::of(b"engine")).unwrap();
    assert!(
        ["exploratory", "mock-gated", "sim-only"]
            .iter()
            .all(|l| v.labels.contains(l)),
        "{:?}",
        v.labels
    );
    assert_eq!(v.slices.len(), 1);
    let s = &v.slices[0];
    assert!(
        s.data
            .cells()
            .iter()
            .all(|c| c.treatment.as_ref().is_some_and(|t| t.completed() == 4))
    );
    assert_eq!(s.data.controls().len(), 1);
    assert_ne!(
        v.verdict,
        V::Inconclusive,
        "a complete set decides: {:?} {:?}",
        v.reasons,
        s.reasons
    );
    // The same bundles, read again and given in another order: the same bytes.
    let mut again = read();
    again.reverse();
    let w = verdict(&h, again, Digest::of(b"engine")).unwrap();
    assert_eq!(v.text(), w.text());
    let path = write(&dir.path().join("runs"), &v).unwrap();
    assert!(path.starts_with(dir.path().join("runs/verdicts")));
    // A tampered bundle does not verify.
    let victim = dirs[0].join("views/call.parquet");
    let mut bytes = std::fs::read(&victim).unwrap();
    let last = bytes.len() - 20;
    bytes[last] ^= 1;
    std::fs::write(&victim, bytes).unwrap();
    assert!(acn_hyp::read::read(&dirs[0]).is_err());
    // Another hypothesis's bundles are refused.
    let other = dir.path().join("zz-other.toml");
    std::fs::write(
        &other,
        HYP.replace("the shipped default", "another default"),
    )
    .unwrap();
    let o = acn_hyp::load_in(&other, dir.path()).unwrap();
    let e = verdict(
        &o,
        dirs[1..]
            .iter()
            .map(|d| acn_hyp::read::read(d).unwrap())
            .collect(),
        Digest::of(b"engine"),
    )
    .unwrap_err();
    assert!(e.to_string().contains("hypothesis.hash"), "{e}");
}
