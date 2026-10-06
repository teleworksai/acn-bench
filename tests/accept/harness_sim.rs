//! SPEC 040 acceptance: a sim run of the smoke workload, twice, yields
//! byte-identical bundles that `acn bundle verify --views` accepts (HAR-50..52,
//! HAR-41, TRC-24), on the embedded mock profiles.

#![allow(clippy::unwrap_used, clippy::expect_used, clippy::panic)] // CON-19: tests are exempt

use std::collections::BTreeMap;
use std::path::{Path, PathBuf};

use acn_harness::agent::Opts;
use acn_harness::run::{HypothesisArg, RunConfig, run};
use acn_harness::wire::Backend;
use acn_trace::bundle;
use acn_trace::identity::{BuildParts, Digest, Mode};

fn smoke() -> PathBuf {
    PathBuf::from(concat!(
        env!("CARGO_MANIFEST_DIR"),
        "/../../workloads/harness-smoke.toml"
    ))
}

fn cfg(runs: &Path, model: &str, vary: &[(&str, &str)]) -> RunConfig {
    RunConfig {
        workload: smoke(),
        backend: Backend::Mockllm,
        model: model.into(),
        mode: Mode::Sim,
        arm: "treatment".into(),
        replicates: 3,
        vary: vary
            .iter()
            .map(|(k, v)| ((*k).to_owned(), (*v).to_owned()))
            .collect::<BTreeMap<_, _>>(),
        opts: Opts::default(),
        hypothesis: HypothesisArg::None { seed: 20_261_002 },
        runs_dir: runs.to_path_buf(),
        start_dir: runs.to_path_buf(),
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

fn files(dir: &Path) -> BTreeMap<String, Vec<u8>> {
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
            if p.is_dir() {
                if rel != bundle::LOGS {
                    stack.push(p);
                }
            } else {
                out.insert(rel, std::fs::read(&p).unwrap());
            }
        }
    }
    out
}

/// Cites: HAR-50, HAR-51, HAR-41, TRC-24, CON-5, MLM-60
#[test]
fn two_sim_runs_of_the_smoke_workload_are_byte_identical() {
    for (model, vary) in [
        ("mock-explicit", &[][..]),
        (
            "mock-auto",
            &[
                ("fanout_prompting", "fork_from_prefix"),
                ("tool_order_stable", "false"),
            ][..],
        ),
        ("mock-blocks", &[("backfill_mode", "tail_restate")][..]),
    ] {
        let (a, b) = (tempfile::tempdir().unwrap(), tempfile::tempdir().unwrap());
        let wa = run(&cfg(a.path(), model, vary)).unwrap();
        let wb = run(&cfg(b.path(), model, vary)).unwrap();
        assert_eq!(wa.run_id, wb.run_id, "{model}");
        assert_eq!(wa.bundle_digest, wb.bundle_digest, "{model}");
        let (fa, fb) = (files(&wa.dir), files(&wb.dir));
        assert_eq!(fa, fb, "{model}: every file but logs/");
        bundle::verify_views(&wa.dir).unwrap();
        let m: serde_json::Value = serde_json::from_slice(&fa[bundle::MANIFEST]).unwrap();
        assert_eq!(m["backend"], "mockllm", "MLM-60");
        assert_eq!(m["model"], model);
        assert!(m.get("endpoint_host").is_none(), "no host in sim (CON-26)");
        assert_eq!(m["params"]["replicates"], "3");
    }
}

/// Cites: HAR-52
#[test]
fn sim_is_the_mock_only_and_netem_is_not_yet_defined() {
    let d = tempfile::tempdir().unwrap();
    let mut c = cfg(d.path(), "mock-auto", &[]);
    c.backend = Backend::Anthropic;
    assert!(run(&c).unwrap_err().to_string().contains("HAR-52"));
    let mut c = cfg(d.path(), "mock-auto", &[]);
    c.mode = Mode::Netem;
    assert!(run(&c).unwrap_err().to_string().contains("HAR-52"));
    assert!(
        std::fs::read_dir(d.path()).unwrap().next().is_none(),
        "nothing written"
    );
}

/// A model, a workload and the knobs varied.
type Case = (
    &'static str,
    PathBuf,
    &'static [(&'static str, &'static str)],
);

/// The digest of every file of a bundle but `logs/`, in path order.
fn tree_digest(dir: &Path) -> String {
    let mut h = blake3::Hasher::new();
    for (path, bytes) in files(dir) {
        h.update(path.as_bytes());
        h.update(&[0]);
        h.update(&(bytes.len() as u64).to_le_bytes());
        h.update(&bytes);
    }
    h.finalize().to_hex().to_string()
}

/// The sim scheduler's behaviour, pinned: the bundles of fixed runs (fixed
/// build parts, so the manifest does not move with the code) that exercise
/// waits, retries and same-instant batches of forked children. A change to the
/// scheduler that changes any of these bytes is a change to every sim run
/// (SPEC 020 EMU-39, T11.2).
///
/// Cites: HAR-41, TRC-24, EMU-39
#[test]
fn sim_bundles_without_a_scenario_are_pinned() {
    let fanout = PathBuf::from(concat!(
        env!("CARGO_MANIFEST_DIR"),
        "/../../workloads/p4-fanout.toml"
    ));
    let cases: [Case; 3] = [
        ("mock-explicit", smoke(), &[]),
        (
            "mock-auto",
            fanout.clone(),
            &[("fanout_prompting", "fork_from_prefix")],
        ),
        ("mock-blocks", fanout, &[("fanout_prompting", "per_child")]),
    ];
    let mut got = Vec::new();
    for (model, workload, vary) in cases {
        let d = tempfile::tempdir().unwrap();
        let mut c = cfg(d.path(), model, vary);
        c.workload = workload;
        c.replicates = 2;
        let w = run(&c).unwrap();
        got.push(tree_digest(&w.dir));
    }
    assert_eq!(got, PINNED, "{got:#?}");
}

const PINNED: [&str; 3] = [
    "870dca80719bed383e9bc6546623879046bf60f945847b9bdf044632547b3da9",
    "52475bde2d18c5088835bef35c8b4ddcaeec1b6b2382426d7e26be16128dd6b0",
    "15a1a1cfbcd61ed41b56cd664029ee248b8ee7f26069b990e7114b62eb37afb1",
];
