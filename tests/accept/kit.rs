//! SPEC 140 acceptance (P16-2 to P16-6, CON-5(c), CON-29): a `sim` bundle is
//! regenerated from its `run_id` alone, its inputs found by hash, byte for
//! byte on the same build; every refusal names its reason.

#![allow(clippy::unwrap_used, clippy::expect_used, clippy::panic)] // CON-19: tests are exempt

use std::collections::BTreeMap;
use std::path::{Path, PathBuf};

use acn_cli::regen::{Regen, regenerate};
use acn_harness::agent::Opts;
use acn_harness::run::{HypothesisArg, RunConfig, run_with_scenario};
use acn_harness::wire::Backend;
use acn_trace::identity::{BuildInfo, BuildParts, Digest, Mode};
use serde_json::Value;

const SHEET: &str = r#"schema_version = 1
placeholder = true
doc = "the kit's sheet"
model = "mock-agentic"
sessions = 2
system_tokens = 50
summary_instruction_tokens = 8
summary_max_tokens = 32
compact_at_tokens = 0
session_start_ns = { uniform = [0, 1_000_000_000] }
turns_per_session = { const = 2 }
think_time_ns = { const = 500_000_000 }
chain_length = { uniform = [1, 2] }
fanout_width = { const = 0 }
user_tokens = { const = 10 }
answer_tokens = { const = 16 }
tool_class = { weighted = [["file", 1]] }

[tool_result_tokens]
file = { const = 20 }

[tool_duration_ns]
file = { const = 1_000_000 }
"#;

fn build(tag: &str) -> BuildInfo {
    BuildParts {
        cargo_lock: Digest::of(b"lock"),
        rust_toolchain: Digest::of(b"toolchain"),
        cargo_config: Digest::of(b"config"),
        source_hash: Digest::of(tag.as_bytes()),
        target: "test",
        profile: "debug",
        features: "",
        rustflags: "",
    }
    .info()
    .unwrap()
}

/// A base directory with the inputs where P16-3 looks for them.
fn base() -> tempfile::TempDir {
    let d = tempfile::tempdir().unwrap();
    let root = Path::new(env!("CARGO_MANIFEST_DIR")).join("../..");
    std::fs::create_dir_all(d.path().join("workloads")).unwrap();
    std::fs::copy(
        root.join("workloads/harness-smoke.toml"),
        d.path().join("workloads/smoke.toml"),
    )
    .unwrap();
    std::fs::write(d.path().join("workloads/sheet.toml"), SHEET).unwrap();
    // The scenarios, the trace-driven one with its trace beside it.
    for sub in ["synthetic", "measured/5g-iana-2023-01-29"] {
        let from = root.join("scenarios").join(sub);
        let to = d.path().join("scenarios").join(sub);
        std::fs::create_dir_all(&to).unwrap();
        for e in std::fs::read_dir(&from).unwrap() {
            let e = e.unwrap();
            if e.file_type().unwrap().is_file() {
                std::fs::copy(e.path(), to.join(e.file_name())).unwrap();
            }
        }
    }
    d
}

fn harness(dir: &Path, scenario: Option<&str>, seed: u64) -> String {
    let cfg = RunConfig {
        workload: dir.join("workloads/smoke.toml"),
        backend: Backend::Mockllm,
        model: "mock-auto".into(),
        mode: Mode::Sim,
        arm: "treatment".into(),
        replicates: 2,
        vary: BTreeMap::from([("fanout_prompting".to_owned(), "per_child".to_owned())]),
        opts: Opts {
            max_retries: 2,
            ..Opts::default()
        },
        hypothesis: HypothesisArg::None { seed },
        runs_dir: dir.join("runs"),
        start_dir: dir.to_path_buf(),
        engine_hash: Digest::of(b"engine"),
        build: build("build"),
        profiles: None,
    };
    let sc = scenario.map(|s| dir.join("scenarios/synthetic").join(s));
    run_with_scenario(&cfg, sc.as_deref())
        .unwrap()
        .run_id
        .to_hex()
}

fn generator(dir: &Path, scenario: Option<&str>) -> String {
    let cfg = acn_gen::run::GenConfig {
        sheet: dir.join("workloads/sheet.toml"),
        mode: Mode::Sim,
        arm: "treatment".into(),
        replicates: 2,
        vary: BTreeMap::new(),
        opts: Opts::default(),
        hypothesis: HypothesisArg::None { seed: 9 },
        runs_dir: dir.join("runs"),
        start_dir: dir.to_path_buf(),
        engine_hash: Digest::of(b"engine"),
        build: build("build"),
        profiles: None,
    };
    let sc = scenario.map(|s| dir.join("scenarios/synthetic").join(s));
    acn_gen::run::run(&cfg, sc.as_deref())
        .unwrap()
        .written
        .run_id
        .to_hex()
}

fn regen_with(dir: &Path, run_id: &str, build_tag: &str, engine: &[u8], across: bool) -> Value {
    regenerate(&Regen {
        run_id,
        runs_dir: Path::new("runs"),
        across_builds: across,
        start_dir: dir,
        engine_hash: Digest::of(engine),
        build: build(build_tag),
    })
}

fn regen(dir: &Path, run_id: &str) -> Value {
    regen_with(dir, run_id, "build", b"engine", false)
}

/// Every file of a directory with its bytes.
fn snapshot(dir: &Path) -> BTreeMap<PathBuf, Vec<u8>> {
    let mut out = BTreeMap::new();
    let mut stack = vec![dir.to_path_buf()];
    while let Some(d) = stack.pop() {
        for e in std::fs::read_dir(&d).unwrap() {
            let p = e.unwrap().path();
            if p.is_dir() {
                stack.push(p);
            } else {
                out.insert(p.clone(), std::fs::read(&p).unwrap());
            }
        }
    }
    out
}

fn identical(v: &Value) {
    assert_eq!(v["ok"], true, "{v}");
    assert_eq!(
        (&v["same_build"], &v["identical"]),
        (&true.into(), &true.into()),
        "{v}"
    );
    assert_eq!(v["differ"], Value::Array(vec![]), "{v}");
}

/// Cites: P16-2, P16-3, P16-4, P16-6, CON-5, CON-29
#[test]
fn harness_and_generator_bundles_regenerate_byte_for_byte_from_their_run_ids() {
    let d = base();
    let dir = d.path();
    let ids = [
        harness(dir, None, 7),
        harness(dir, Some("cellular-handover.toml"), 7),
        harness(dir, Some("5g-iana-replay.toml"), 7),
        generator(dir, None),
        generator(dir, Some("clean.toml")),
    ];
    for id in &ids {
        let original = dir.join("runs").join(id);
        let before = snapshot(&original);
        let v = regen(dir, id);
        identical(&v);
        // The regenerated bundle is a scratch copy at regen/<run_id>/1/<run_id>/.
        let at = dir.join("runs/regen").join(id).join("1").join(id);
        assert_eq!(
            Path::new(v["dir"].as_str().unwrap())
                .canonicalize()
                .unwrap(),
            at.canonicalize().unwrap()
        );
        assert_eq!(
            std::fs::read(at.join("manifest.json")).unwrap(),
            std::fs::read(original.join("manifest.json")).unwrap()
        );
        // The original is untouched, and a second regeneration takes n = 2.
        assert_eq!(snapshot(&original), before);
        let v = regen(dir, id);
        identical(&v);
        assert!(v["dir"].as_str().unwrap().contains("/2/"), "{v}");
    }
}

/// Cites: P16-2, P16-6
#[test]
fn a_reference_manifest_alone_regenerates() {
    let d = base();
    let dir = d.path();
    let id = harness(dir, Some("clean.toml"), 3);
    std::fs::create_dir_all(dir.join("kit/manifests")).unwrap();
    std::fs::copy(
        dir.join("runs").join(&id).join("manifest.json"),
        dir.join("kit/manifests").join(format!("{id}.json")),
    )
    .unwrap();
    std::fs::rename(dir.join("runs").join(&id), dir.join("elsewhere")).unwrap();
    identical(&regen(dir, &id));
    // A reference manifest that is not canonical is refused.
    let path = dir.join("kit/manifests").join(format!("{id}.json"));
    let text = std::fs::read_to_string(&path).unwrap();
    std::fs::write(&path, text.replacen(',', ", ", 1)).unwrap();
    let v = regen(dir, &id);
    assert_eq!(
        (&v["ok"], v["code"].as_str()),
        (&false.into(), Some("manifest_invalid")),
        "{v}"
    );
}

/// Cites: P16-5, P16-6
#[test]
fn another_build_is_refused_unless_compared_without_its_build() {
    let d = base();
    let dir = d.path();
    let id = harness(dir, Some("clean.toml"), 4);
    let v = regen_with(dir, &id, "other", b"engine", false);
    assert_eq!(v["code"], "not_regenerable_with_this_build", "{v}");
    // Across builds: the bundle agrees in build-neutral form, but identity is
    // not claimed on the same footing.
    let v = regen_with(dir, &id, "other", b"engine", true);
    assert_eq!(v["ok"], true, "{v}");
    assert_eq!(
        (&v["same_build"], &v["identical"]),
        (&false.into(), &true.into()),
        "{v}"
    );
    // Another engine makes another run (CON-29).
    let v = regen_with(dir, &id, "build", b"other engine", false);
    assert_eq!(v["code"], "run_id_differs", "{v}");
}

/// Cites: P16-2, P16-3, P16-4
#[test]
fn every_refusal_names_its_reason() {
    let d = base();
    let dir = d.path();
    let id = harness(dir, None, 5);
    // A changed workload is no longer found by its hash.
    let w = dir.join("workloads/smoke.toml");
    let text = std::fs::read_to_string(&w).unwrap();
    std::fs::write(&w, format!("{text}\n# edited\n")).unwrap();
    let v = regen(dir, &id);
    assert_eq!(v["code"], "input_missing", "{v}");
    assert!(v["hash"].is_string(), "{v}");
    // A copy anywhere under the roots is found; of two, the first by path.
    std::fs::create_dir_all(dir.join("lab/x")).unwrap();
    std::fs::write(dir.join("lab/x/w.toml"), &text).unwrap();
    std::fs::write(dir.join("lab/x/z.toml"), &text).unwrap();
    identical(&regen(dir, &id));
    // A file under a `target` directory is not an input.
    std::fs::remove_file(dir.join("lab/x/w.toml")).unwrap();
    std::fs::remove_file(dir.join("lab/x/z.toml")).unwrap();
    std::fs::create_dir_all(dir.join("lab/x/target")).unwrap();
    std::fs::write(dir.join("lab/x/target/w.toml"), &text).unwrap();
    assert_eq!(regen(dir, &id)["code"], "input_missing");
    std::fs::write(&w, &text).unwrap();
    // A bundle that does not verify is refused, never replaced.
    let events = dir.join("runs").join(&id).join("events.parquet");
    let bytes = std::fs::read(&events).unwrap();
    std::fs::write(&events, b"changed").unwrap();
    assert_eq!(regen(dir, &id)["code"], "bundle_invalid");
    std::fs::write(&events, bytes).unwrap();
    // An unknown run.
    assert_eq!(regen(dir, &"0".repeat(64))["code"], "unknown_run");
}

/// Cites: P16-2
#[test]
fn a_live_bundle_is_not_reproduced_byte_for_byte() {
    let d = base();
    let dir = d.path();
    let cfg = RunConfig {
        workload: dir.join("workloads/smoke.toml"),
        backend: Backend::Mockllm,
        model: "mock-auto".into(),
        mode: Mode::Live,
        arm: "treatment".into(),
        replicates: 1,
        vary: BTreeMap::new(),
        opts: Opts {
            endpoint: acn_harness::served::LOOPBACK.into(),
            request_timeout_ms: 10_000,
            ..Opts::default()
        },
        hypothesis: HypothesisArg::None { seed: 1 },
        runs_dir: dir.join("runs"),
        start_dir: dir.to_path_buf(),
        engine_hash: Digest::of(b"engine"),
        build: build("build"),
        profiles: None,
    };
    let id = run_with_scenario(&cfg, None).unwrap().run_id.to_hex();
    assert_eq!(regen(dir, &id)["code"], "not_reproducible_mode");
}

/// Cites: P16-3
#[test]
fn a_hypothesis_is_found_only_with_the_status_the_run_recorded() {
    let d = base();
    let dir = d.path();
    std::fs::create_dir_all(dir.join("lab/hypotheses")).unwrap();
    let h = dir.join("lab/hypotheses/zz.toml");
    std::fs::write(
        &h,
        r#"[poc]
id = "zz"
title = "a kit test"

[hypothesis]
statement = "s"

[varies]
tool_order_stable = { kind = "bool" }

[measures]
primary = ["cached_token_ratio"]

[control]
description = "the default"
config = { tool_order_stable = true }

[design]
search = "grid"
replicates = 4
twin_required = false
seed = 42

[falsifier]
predicate = "max_over_knobs(abs(effect(cached_token_ratio))) < 0.5"

[expected]
outcome = "pass"
"#,
    )
    .unwrap();
    let cfg = RunConfig {
        workload: dir.join("workloads/smoke.toml"),
        backend: Backend::Mockllm,
        model: "mock-auto".into(),
        mode: Mode::Sim,
        arm: "treatment".into(),
        replicates: 1,
        vary: BTreeMap::from([("tool_order_stable".to_owned(), "false".to_owned())]),
        opts: Opts::default(),
        hypothesis: HypothesisArg::File(h.clone()),
        runs_dir: dir.join("runs"),
        start_dir: dir.to_path_buf(),
        engine_hash: Digest::of(b"engine"),
        build: build("build"),
        profiles: None,
    };
    let id = run_with_scenario(&cfg, None).unwrap().run_id.to_hex();
    identical(&regen(dir, &id));
    // Under hypotheses/ the same bytes would be frozen: not the candidate the
    // run recorded, so it is not found there.
    std::fs::create_dir_all(dir.join("hypotheses")).unwrap();
    std::fs::rename(&h, dir.join("hypotheses/zz.toml")).unwrap();
    let v = regen(dir, &id);
    assert_eq!(v["code"], "input_missing", "{v}");
}

/// Cites: P16-4
#[test]
fn a_manifest_of_an_unknown_producer_is_refused() {
    let d = base();
    let dir = d.path();
    let id = harness(dir, None, 6);
    let bytes = std::fs::read(dir.join("runs").join(&id).join("manifest.json")).unwrap();
    let mut m: acn_trace::bundle::Manifest = serde_json::from_slice(&bytes).unwrap();
    m.producers = vec!["acn-emu".into()];
    std::fs::create_dir_all(dir.join("kit/manifests")).unwrap();
    std::fs::write(
        dir.join("kit/manifests").join(format!("{id}.json")),
        m.to_bytes().unwrap(),
    )
    .unwrap();
    std::fs::rename(dir.join("runs").join(&id), dir.join("elsewhere")).unwrap();
    let v = regen(dir, &id);
    assert_eq!(v["code"], "unknown_producer", "{v}");
}

/// Cites: P16-1
#[test]
fn poc_16_has_no_hypothesis_file() {
    let root = Path::new(env!("CARGO_MANIFEST_DIR")).join("../..");
    for d in ["hypotheses", "lab/hypotheses"] {
        let found: Vec<_> = std::fs::read_dir(root.join(d))
            .unwrap()
            .filter_map(Result::ok)
            .filter(|e| e.file_name().to_string_lossy().starts_with("p16"))
            .collect();
        assert!(
            found.is_empty(),
            "POC 16 is records, not a verdict (ADR-41)"
        );
    }
}

/// Replace `file` of the bundle with `bytes`, and its hash in the manifest, so
/// that the bundle still verifies: a bundle that differs and is valid.
fn tamper(bundle: &Path, file: &str, bytes: &[u8]) {
    std::fs::write(bundle.join(file), bytes).unwrap();
    let m = bundle.join("manifest.json");
    let mut man: acn_trace::bundle::Manifest =
        serde_json::from_slice(&std::fs::read(&m).unwrap()).unwrap();
    man.files
        .insert(file.into(), blake3::hash(bytes).to_hex().to_string());
    std::fs::write(&m, man.to_bytes().unwrap()).unwrap();
    acn_trace::bundle::verify(bundle).unwrap();
}

/// Cites: P16-6
#[test]
fn a_bundle_that_differs_is_never_reported_identical() {
    let d = base();
    let dir = d.path();
    let id = harness(dir, None, 8);
    let original = dir.join("runs").join(&id);
    // Another valid copy of a table (its rows reversed would do; any other
    // valid Parquet file does): the original's links file from another run.
    let other = harness(dir, Some("clean.toml"), 8);
    let bytes = std::fs::read(dir.join("runs").join(&other).join("links.parquet")).unwrap();
    tamper(&original, "links.parquet", &bytes);
    let v = regen(dir, &id);
    assert_eq!(
        (&v["ok"], &v["identical"]),
        (&false.into(), &false.into()),
        "{v}"
    );
    assert_eq!(
        v["differ"],
        serde_json::json!(["links.parquet", "manifest.json"]),
        "{v}"
    );
    // Across builds it is completed, and still not identical.
    let v = regen_with(dir, &id, "other", b"engine", true);
    assert_eq!(
        (&v["ok"], &v["identical"]),
        (&true.into(), &false.into()),
        "{v}"
    );
    assert!(
        v["differ"]
            .as_array()
            .unwrap()
            .iter()
            .any(|p| p == "links.parquet"),
        "{v}"
    );
    // From the reference manifest alone, too.
    std::fs::create_dir_all(dir.join("kit/manifests")).unwrap();
    std::fs::copy(
        original.join("manifest.json"),
        dir.join("kit/manifests").join(format!("{id}.json")),
    )
    .unwrap();
    std::fs::rename(&original, dir.join("elsewhere")).unwrap();
    let v = regen(dir, &id);
    assert_eq!(
        (&v["ok"], &v["identical"]),
        (&false.into(), &false.into()),
        "{v}"
    );
}

/// Cites: P16-2, P16-5, P16-6
#[test]
fn identities_and_directories_are_checked() {
    let d = base();
    let dir = d.path();
    let id = harness(dir, None, 10);
    // Not a run_id at all.
    for bad in ["", "../../x", &id.to_uppercase()] {
        assert_eq!(regen(dir, bad)["code"], "unknown_run", "{bad}");
    }
    // A canonical manifest whose run_id does not recompute.
    let mut m: acn_trace::bundle::Manifest = serde_json::from_slice(
        &std::fs::read(dir.join("runs").join(&id).join("manifest.json")).unwrap(),
    )
    .unwrap();
    m.seed = "11".into();
    std::fs::create_dir_all(dir.join("kit/manifests")).unwrap();
    let other = "f".repeat(64);
    std::fs::write(
        dir.join("kit/manifests").join(format!("{other}.json")),
        m.to_bytes().unwrap(),
    )
    .unwrap();
    assert_eq!(regen(dir, &other)["code"], "manifest_invalid");
    // The smallest unused n, around a gap.
    let parent = dir.join("runs/regen").join(&id);
    std::fs::create_dir_all(parent.join("2")).unwrap();
    let v = regen(dir, &id);
    identical(&v);
    assert!(
        v["dir"].as_str().unwrap().contains(&format!("{id}/1/")),
        "{v}"
    );
    let v = regen(dir, &id);
    identical(&v);
    assert!(
        v["dir"].as_str().unwrap().contains(&format!("{id}/3/")),
        "{v}"
    );
    // --across-builds reaches only one engine (CON-29).
    let v = regen_with(dir, &id, "other", b"other engine", true);
    assert_eq!(v["code"], "run_id_differs", "{v}");
}

/// Cites: P16-6, P16-12
#[test]
fn one_run_made_by_two_builds_has_one_build_neutral_form() {
    let d = base();
    let dir = d.path();
    let id = harness(dir, Some("cellular-handover.toml"), 12);
    let v = regen_with(dir, &id, "other", b"engine", true);
    assert_eq!(
        (&v["ok"], &v["identical"]),
        (&true.into(), &true.into()),
        "{v}"
    );
    let a = acn_cli::regen::neutral(&dir.join("runs").join(&id));
    let b = acn_cli::regen::neutral(Path::new(v["dir"].as_str().unwrap()));
    assert_eq!(
        (&a["ok"], &b["ok"]),
        (&true.into(), &true.into()),
        "{a} {b}"
    );
    assert_ne!(a["build_hash"], b["build_hash"]);
    for k in ["run_id", "files", "resources", "manifest"] {
        assert_eq!(a[k], b[k], "{k}");
    }
    // Nothing that records the build is left in the neutral form.
    let text = format!("{}{}", a["resources"], a["manifest"]);
    assert!(!text.contains(a["build_hash"].as_str().unwrap()), "{text}");
    assert!(!text.contains("acn.build_hash"), "{text}");
    // Only the build is taken out: the rest of each resource row stays.
    let res = a["resources"].to_string();
    assert!(
        res.contains("acn.engine_hash") && res.contains("service.name"),
        "{res}"
    );
}
