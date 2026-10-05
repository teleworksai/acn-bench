//! SPEC 085 acceptance, LOOP-1 and LOOP-2: an L1 chain (loop report →
//! verdict → bundles), made by the real harness in `sim`, verifies from its
//! loop_id and from its verdict_id. Breaking any link fails it: a bundle file,
//! the recorded layer, the verdict's bytes. A verdict no loop report names, and
//! a binary of another build, fail too. The L2 and L3 links come with T11b and
//! T30, and the evidence pages (LOOP-30) with T07.

#![allow(clippy::unwrap_used, clippy::expect_used, clippy::panic)] // CON-19: tests are exempt

use std::path::{Path, PathBuf};

use acn_harness::agent::Opts;
use acn_harness::run::{HypothesisArg, RunConfig, run};
use acn_harness::wire::Backend;
use acn_hyp::evidence::{self, Checked};
use acn_hyp::loop_run::{self, Args, Binary, Code, Completed, Executor, Request};
use acn_trace::identity::{BuildInfo, BuildParts, Digest, Mode};

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

fn build(tag: &str) -> BuildInfo {
    BuildParts {
        cargo_lock: Digest::of(b"lock"),
        rust_toolchain: Digest::of(b"toolchain"),
        cargo_config: Digest::of(b"config"),
        source_hash: Digest::of(tag.as_bytes()),
        target: "accept",
        profile: "debug",
        features: "",
        rustflags: "",
    }
    .info()
    .unwrap()
}

/// The harness on the mock in `sim`, as `acn loop run`'s executor runs it.
struct Harness {
    build: BuildInfo,
}

impl Harness {
    fn bin(&self) -> Binary {
        Binary {
            engine_hash: Digest::of(b"engine"),
            build_hash: Digest::from_hex(&self.build.build_hash).unwrap(),
        }
    }
}

impl Executor for Harness {
    fn run(&mut self, r: &Request) -> Result<PathBuf, String> {
        run(&RunConfig {
            workload: r.workload.clone(),
            backend: Backend::Mockllm,
            model: r.model.clone(),
            mode: Mode::Sim,
            arm: r.arm.as_str().to_owned(),
            replicates: r.replicates,
            vary: r.vary.clone(),
            opts: Opts::default(),
            hypothesis: HypothesisArg::File(r.hypothesis.clone()),
            runs_dir: r.runs_dir.clone(),
            start_dir: r.start_dir.clone(),
            engine_hash: Digest::of(b"engine"),
            build: self.build.clone(),
            profiles: None,
        })
        .map(|w| w.dir)
        .map_err(|e| e.to_string())
    }

    fn check_model(&self, model: &str) -> Result<(), String> {
        acn_mockllm::profile::embedded()
            .map_err(|e| e.to_string())?
            .get(model)
            .map(|_| ())
            .ok_or_else(|| "not a mock profile".into())
    }

    fn check_workload(&self, path: &Path) -> Result<(), String> {
        acn_harness::workload::Workload::load(path)
            .map(|_| ())
            .map_err(|e| e.to_string())
    }
}

fn harness() -> Harness {
    Harness {
        build: build("accept"),
    }
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
    loop_run::run(&h, &args, &dir.join("runs"), bin, &mut x).unwrap()
}

fn check(dir: &Path, id: &Digest) -> Checked {
    let mut x = harness();
    let bin = x.bin();
    evidence::verify(&dir.join("runs"), &id.to_hex(), bin, &mut x).unwrap()
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
        loop_run::run(&h, &args, &d.join("runs"), bin, &mut x).unwrap()
    };
    let k = check(d, &c.verdict_id);
    assert!(k.ok(), "{:?}", k.findings);
    let mut both = vec![c.loop_id.to_hex(), other.loop_id.to_hex()];
    both.sort();
    assert_eq!(k.loops, both);
}

/// Cites: LOOP-2, LOOP-1
#[test]
fn breaking_any_link_fails_the_chain() {
    // A bundle's file.
    let dir = tempfile::tempdir().unwrap();
    let d = dir.path();
    let c = chain(d, 10);
    let first = c.run_ids[0].to_hex();
    std::fs::write(d.join("runs").join(&first).join("spans.parquet"), b"x").unwrap();
    let k = check(d, &c.loop_id);
    assert!(!k.ok());
    assert!(codes(&k).contains(&Code::BundleInvalid), "{:?}", k.findings);
    assert!(
        codes(&k).contains(&Code::NotRegenerated),
        "{:?}",
        k.findings
    );

    // The layer a report records.
    let dir = tempfile::tempdir().unwrap();
    let d = dir.path();
    let c = chain(d, 10);
    let text = std::fs::read_to_string(&c.report).unwrap();
    std::fs::write(
        &c.report,
        text.replace("\"layer\":\"L1\"", "\"layer\":\"L2\""),
    )
    .unwrap();
    let k = check(d, &c.loop_id);
    assert!(codes(&k).contains(&Code::LayerMismatch), "{:?}", k.findings);

    // The verdict's bytes.
    let dir = tempfile::tempdir().unwrap();
    let d = dir.path();
    let c = chain(d, 10);
    let vpath = d
        .join("runs/verdicts")
        .join(c.verdict_id.to_hex())
        .join("verdict.json");
    std::fs::write(&vpath, "{}\n").unwrap();
    let k = check(d, &c.loop_id);
    assert!(
        codes(&k).contains(&Code::VerdictMismatch),
        "{:?}",
        k.findings
    );

    // Another binary cannot regenerate or recompute (CON-31).
    let dir = tempfile::tempdir().unwrap();
    let d = dir.path();
    let c = chain(d, 10);
    let mut other = Harness {
        build: build("other"),
    };
    let bin = other.bin();
    let k = evidence::verify(&d.join("runs"), &c.loop_id.to_hex(), bin, &mut other).unwrap();
    assert_eq!(codes(&k), [Code::NotRegenerable], "{:?}", k.findings);
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
    let e = evidence::verify(&d.join("runs"), "nope", bin, &mut x).unwrap_err();
    assert_eq!(e.code, Code::Report);
}
