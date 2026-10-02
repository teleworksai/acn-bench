//! TRC-22, TRC-23: the bundle, its manifest and `verify`; CON-29 a bundle is never
//! replaced; CON-26 the endpoint host; TRC-19 resources name the run's engine and
//! build; CON-28 a bundle needs a preflight for its own hypothesis.

#![allow(clippy::unwrap_used, clippy::expect_used, clippy::panic)] // CON-19: tests are exempt

use std::collections::BTreeMap;
use std::path::{Path, PathBuf};

use acn_trace::bundle::{self, Bundle, BundleError, HypothesisRef, Manifest, RunSpec};
use acn_trace::env::{self, Preflight, RunHypothesis};
use acn_trace::fixture::{self, FixtureRun};
use acn_trace::identity::{BuildInfo, BuildParts, Digest, HypStatus, Mode, RunParams, Value};
use acn_trace::model::{AttrValue, Trace};

fn build() -> BuildInfo {
    BuildParts {
        cargo_lock: Digest::of(b"lock"),
        rust_toolchain: Digest::of(b"toolchain"),
        cargo_config: Digest::of(b"config"),
        source_hash: Digest::of(b"source"),
        target: "aarch64-apple-darwin",
        profile: "debug",
        features: "",
        rustflags: "",
    }
    .info()
    .unwrap()
}

const ENGINE: &[u8] = b"engine";

fn preflight(h: RunHypothesis) -> Preflight {
    let outside = tempfile::tempdir().unwrap();
    env::preflight(outside.path(), Digest::of(ENGINE), h).unwrap()
}

fn params() -> RunParams {
    RunParams {
        backend: "mockllm".into(),
        model: "mock-default".into(),
        hyp_status: HypStatus::Candidate,
        arms: vec!["treatment".into()],
        replicates: 1,
        vary: BTreeMap::from([("ratio".into(), Value::Float(0.5))]),
        opts: BTreeMap::new(),
    }
}

fn spec(seed: u64) -> RunSpec {
    RunSpec {
        seed,
        mode: Mode::Sim,
        scenario_hash: Digest::of(b"scenario"),
        workload_hash: Digest::of(b"workload"),
        hypothesis: HypothesisRef::none(),
        params: params(),
        endpoint_host: None,
        execution_order: None,
        started_at: None,
    }
}

fn session(run_id: &str, seed: u64) -> Trace {
    fixture::session(&FixtureRun {
        run_id: run_id.into(),
        seed,
        replicate: 0,
        engine_hash: Digest::of(ENGINE),
        build_hash: Digest::from_hex(&build().build_hash).unwrap(),
    })
    .unwrap()
}

fn write(runs: &Path, seed: u64) -> bundle::Written {
    let b = Bundle::create(runs, &preflight(RunHypothesis::None), &build(), spec(seed)).unwrap();
    let t = session(b.run_id(), seed);
    std::fs::write(b.logs_dir().join("stderr.log"), "noise").unwrap();
    b.finish(&t).unwrap()
}

fn manifest(dir: &Path) -> (Vec<u8>, Manifest) {
    let bytes = std::fs::read(dir.join(bundle::MANIFEST)).unwrap();
    let m = serde_json::from_slice(&bytes).unwrap();
    (bytes, m)
}

/// Rewrite the manifest canonically after an edit, as a forger would.
fn forge(dir: &Path, edit: impl FnOnce(&mut Manifest)) {
    let (_, mut m) = manifest(dir);
    edit(&mut m);
    std::fs::write(dir.join(bundle::MANIFEST), m.to_bytes().unwrap()).unwrap();
}

/// Cites: TRC-22, TRC-23
#[test]
fn a_bundle_lists_every_file_but_its_logs_and_verifies() {
    let runs = tempfile::tempdir().unwrap();
    let w = write(runs.path(), 1);
    assert_eq!(w.dir, runs.path().join(w.run_id.to_hex()), "runs/<run_id>/");
    let (bytes, m) = manifest(&w.dir);
    assert_eq!(
        m.files.keys().map(String::as_str).collect::<Vec<_>>(),
        [
            "events.parquet",
            "links.parquet",
            "resources.parquet",
            "spans.parquet"
        ],
        "logs/ is neither hashed nor listed"
    );
    assert_eq!(
        w.bundle_digest,
        Digest::of(&bytes),
        "bundle_digest is the manifest's hash"
    );
    let v = bundle::verify(&w.dir).unwrap();
    assert_eq!((v.run_id, v.bundle_digest), (w.run_id, w.bundle_digest));
    assert_eq!(m.producers, ["acn-harness"]);
    assert_eq!(
        m.semconv_version,
        acn_trace::schema::inventory().unwrap().semconv_version()
    );
    assert_eq!(m.seed, "1");
    assert_eq!(m.endpoint_host, None, "the mock in sim has no endpoint");
    assert_eq!(m.params["vary.ratio"], "0.5");
}

/// Cites: TRC-23
#[test]
fn the_manifest_is_canonical_json() {
    let runs = tempfile::tempdir().unwrap();
    let w = write(runs.path(), 2);
    let (bytes, _) = manifest(&w.dir);
    let text = String::from_utf8(bytes).unwrap();
    assert!(text.ends_with("}\n") && !text[..text.len() - 1].contains('\n'));
    assert!(
        !text.contains(": ") && !text.contains(", "),
        "no insignificant whitespace"
    );
    let v: serde_json::Value = serde_json::from_str(&text).unwrap();
    // serde_json's own map is ordered by key (no `preserve_order`), and the manifest
    // holds no floats, so its compact form is an independent canonical reference.
    assert_eq!(
        text,
        serde_json::to_string(&v).unwrap() + "\n",
        "sorted keys, compact"
    );
    // A re-indented manifest no longer verifies.
    let pretty = serde_json::to_string_pretty(&v).unwrap() + "\n";
    std::fs::write(w.dir.join(bundle::MANIFEST), pretty).unwrap();
    assert!(bundle::verify(&w.dir).is_err());
}

/// Cites: TRC-23
#[test]
fn a_changed_missing_or_unlisted_file_fails_verification() {
    let runs = tempfile::tempdir().unwrap();
    let w = write(runs.path(), 3);
    std::fs::write(w.dir.join("logs/more.log"), "logs may change").unwrap();
    assert!(bundle::verify(&w.dir).is_ok());

    std::fs::write(w.dir.join("extra.txt"), "x").unwrap();
    assert!(
        bundle::verify(&w.dir).is_err(),
        "neither listed nor under logs/"
    );
    std::fs::remove_file(w.dir.join("extra.txt")).unwrap();

    std::fs::create_dir(w.dir.join(bundle::SIDECAR)).unwrap();
    std::fs::write(w.dir.join("sidecar/raw.json"), "{}").unwrap();
    assert!(
        bundle::verify(&w.dir).is_err(),
        "a sidecar file must be listed too"
    );
    std::fs::remove_dir_all(w.dir.join(bundle::SIDECAR)).unwrap();

    let spans = w.dir.join("spans.parquet");
    let original = std::fs::read(&spans).unwrap();
    let mut changed = original.clone();
    let last = changed.len() - 9;
    changed[last] ^= 1;
    std::fs::write(&spans, &changed).unwrap();
    assert!(bundle::verify(&w.dir).is_err(), "a changed table");
    std::fs::remove_file(&spans).unwrap();
    assert!(bundle::verify(&w.dir).is_err(), "a missing table");
    std::fs::write(&spans, &original).unwrap();
    assert!(bundle::verify(&w.dir).is_ok());
}

/// Cites: TRC-23, CON-29
#[test]
fn verification_recomputes_the_run_id_and_the_build_hash() {
    let runs = tempfile::tempdir().unwrap();
    let w = write(runs.path(), 4);
    // Each edit keeps the manifest otherwise consistent, so that only the identity
    // recomputation can catch it.
    type Edit = Box<dyn Fn(&mut Manifest)>;
    let identity_cases: Vec<(&str, Edit)> = vec![
        ("seed", Box::new(|m| m.seed = "5".into())),
        (
            "scenario",
            Box::new(|m| m.scenario_hash = Digest::of(b"o").to_hex()),
        ),
        (
            "workload",
            Box::new(|m| m.workload_hash = Digest::of(b"o").to_hex()),
        ),
        (
            "engine",
            Box::new(|m| m.engine_hash = Digest::of(b"o").to_hex()),
        ),
        (
            "mode",
            Box::new(|m| {
                m.mode = "live".into();
                m.execution_order = Some(vec!["treatment/0".into()]);
                m.started_at = Some("2026-10-02T00:00:00Z".into());
            }),
        ),
        (
            "a parameter",
            Box::new(|m| {
                m.params.insert("vary.ratio".into(), "0.6".into());
            }),
        ),
        (
            "the hypothesis",
            Box::new(|m| {
                m.hypothesis.id = "p9".into();
                m.hypothesis.hash = Digest::of(b"p9.toml").to_hex();
            }),
        ),
        (
            "a status claim",
            Box::new(|m| {
                m.hypothesis.id = "p9".into();
                m.hypothesis.hash = Digest::of(b"p9.toml").to_hex();
                m.hypothesis.status = "frozen".into();
                m.params.insert("hyp_status".into(), "frozen".into());
            }),
        ),
    ];
    let (original, _) = manifest(&w.dir);
    for (what, edit) in identity_cases {
        forge(&w.dir, edit);
        let err = bundle::verify(&w.dir).unwrap_err().to_string();
        assert!(err.contains("run_id"), "{what}: {err}");
        std::fs::write(w.dir.join(bundle::MANIFEST), &original).unwrap();
    }
    forge(&w.dir, |m| m.build.profile = "release".into());
    let err = bundle::verify(&w.dir).unwrap_err().to_string();
    assert!(err.contains("build_hash"), "{err}");
    std::fs::write(w.dir.join(bundle::MANIFEST), &original).unwrap();
    forge(&w.dir, |m| m.seed = "04".into());
    assert!(bundle::verify(&w.dir).is_err(), "a non-canonical seed");
    std::fs::write(w.dir.join(bundle::MANIFEST), &original).unwrap();
    // A bundle moved under another name does not verify either.
    let moved = runs.path().join(Digest::of(b"x").to_hex());
    std::fs::rename(&w.dir, &moved).unwrap();
    assert!(bundle::verify(&moved).is_err());
}

/// Cites: CON-29
#[test]
fn a_bundle_is_never_written_into_an_existing_directory() {
    let runs = tempfile::tempdir().unwrap();
    let w = write(runs.path(), 6);
    let err = Bundle::create(
        runs.path(),
        &preflight(RunHypothesis::None),
        &build(),
        spec(6),
    )
    .unwrap_err();
    assert!(matches!(err, BundleError::Refused(_)), "{err}");
    assert!(
        bundle::verify(&w.dir).is_ok(),
        "the first bundle is untouched"
    );
}

/// Cites: CON-26, TRC-22
#[test]
fn a_real_backend_records_its_endpoint_and_the_mock_in_sim_does_not() {
    let runs = tempfile::tempdir().unwrap();
    let pf = preflight(RunHypothesis::None);
    let mut s = spec(7);
    s.params.backend = "anthropic".into();
    assert!(
        Bundle::create(runs.path(), &pf, &build(), s.clone()).is_err(),
        "no endpoint"
    );
    s.endpoint_host = Some("api.anthropic.com".into());
    s.mode = Mode::Live;
    s.execution_order = Some(vec!["treatment/0".into()]);
    s.started_at = Some("2026-10-02T00:00:00Z".into());
    assert!(Bundle::create(runs.path(), &pf, &build(), s).is_ok());

    let mut s = spec(8);
    s.endpoint_host = Some("127.0.0.1".into());
    assert!(
        Bundle::create(runs.path(), &pf, &build(), s).is_err(),
        "mock in sim has none"
    );
}

/// Cites: TRC-22, TRC-26
#[test]
fn live_runs_record_their_order_and_start_and_sim_runs_do_not() {
    let runs = tempfile::tempdir().unwrap();
    let pf = preflight(RunHypothesis::None);
    let mut s = spec(9);
    s.mode = Mode::Live;
    assert!(Bundle::create(runs.path(), &pf, &build(), s).is_err());
    let mut s = spec(10);
    s.started_at = Some("2026-10-02T00:00:00Z".into());
    assert!(Bundle::create(runs.path(), &pf, &build(), s).is_err());
}

/// Cites: TRC-19
#[test]
fn every_resource_names_the_runs_engine_and_build() {
    let runs = tempfile::tempdir().unwrap();
    let pf = preflight(RunHypothesis::None);
    let b = Bundle::create(runs.path(), &pf, &build(), spec(11)).unwrap();
    let mut t = session(b.run_id(), 11);
    t.resources[0].attrs.insert(
        "acn.engine_hash".into(),
        AttrValue::String(Digest::of(b"other").to_hex()),
    );
    assert!(b.finish(&t).is_err());

    let b = Bundle::create(runs.path(), &pf, &build(), spec(12)).unwrap();
    let mut t = session(b.run_id(), 12);
    t.resources[0].attrs.remove("service.version");
    assert!(b.finish(&t).is_err());
}

/// Cites: CON-28, CON-27
#[test]
fn a_bundle_needs_a_preflight_for_its_own_hypothesis() {
    let runs = tempfile::tempdir().unwrap();
    let mut s = spec(13);
    s.hypothesis = HypothesisRef {
        id: "p4".into(),
        hash: Digest::of(b"p4.toml"),
    };
    s.params.hyp_status = HypStatus::Frozen;
    let err = Bundle::create(
        runs.path(),
        &preflight(RunHypothesis::None),
        &build(),
        s.clone(),
    )
    .unwrap_err();
    assert!(matches!(err, BundleError::Refused(_)), "{err}");
    let err = Bundle::create(
        runs.path(),
        &preflight(RunHypothesis::Candidate {
            hash: Digest::of(b"p4.toml"),
        }),
        &build(),
        s,
    )
    .unwrap_err();
    assert!(
        matches!(err, BundleError::Refused(_)),
        "a candidate check for a frozen run"
    );

    // A candidate preflight binds the hash: another file cannot ride on it.
    let mut s = spec(16);
    s.hypothesis = HypothesisRef {
        id: "p17".into(),
        hash: Digest::of(b"edited"),
    };
    let err = Bundle::create(
        runs.path(),
        &preflight(RunHypothesis::Candidate {
            hash: Digest::of(b"checked"),
        }),
        &build(),
        s.clone(),
    )
    .unwrap_err();
    assert!(matches!(err, BundleError::Refused(_)), "{err}");
    s.hypothesis.hash = Digest::of(b"checked");
    assert!(
        Bundle::create(
            runs.path(),
            &preflight(RunHypothesis::Candidate {
                hash: Digest::of(b"checked")
            }),
            &build(),
            s,
        )
        .is_ok()
    );
    // A `none` spec under a candidate preflight is not what was checked either.
    let err = Bundle::create(
        runs.path(),
        &preflight(RunHypothesis::Candidate { hash: Digest::ZERO }),
        &build(),
        spec(17),
    )
    .unwrap_err();
    assert!(matches!(err, BundleError::Refused(_)), "{err}");

    // `none` is 32 zero bytes with status candidate, and nothing else is.
    let mut s = spec(14);
    s.hypothesis.hash = Digest::of(b"x");
    assert!(Bundle::create(runs.path(), &preflight(RunHypothesis::None), &build(), s).is_err());
}

/// Cites: TRC-23
#[test]
fn a_symlink_in_a_bundle_fails_verification() {
    let runs = tempfile::tempdir().unwrap();
    let w = write(runs.path(), 15);
    #[cfg(unix)]
    {
        std::os::unix::fs::symlink("spans.parquet", w.dir.join("alias.parquet")).unwrap();
        assert!(bundle::verify(&w.dir).is_err());
    }
    let _: PathBuf = w.dir;
}

/// Rewrite the manifest after `edit` with a recomputed `run_id`, and move the
/// bundle to the matching directory: a forger who knows the encoding.
fn reidentify(dir: &Path, edit: impl FnOnce(&mut Manifest)) -> PathBuf {
    use acn_trace::identity::{self, RunIdentity};
    let (_, mut m) = manifest(dir);
    edit(&mut m);
    m.run_id = RunIdentity {
        seed: m.seed.parse().unwrap(),
        scenario_hash: Digest::from_hex(&m.scenario_hash).unwrap(),
        workload_hash: Digest::from_hex(&m.workload_hash).unwrap(),
        hypothesis_hash: Digest::from_hex(&m.hypothesis.hash).unwrap(),
        engine_hash: Digest::from_hex(&m.engine_hash).unwrap(),
        mode: Mode::parse(&m.mode).unwrap(),
        params_hash: identity::params_hash(&m.params).unwrap(),
    }
    .run_id()
    .unwrap()
    .to_hex();
    std::fs::write(dir.join(bundle::MANIFEST), m.to_bytes().unwrap()).unwrap();
    let to = dir.parent().unwrap().join(&m.run_id);
    std::fs::rename(dir, &to).unwrap();
    to
}

/// Cites: CON-29, TRC-23
#[test]
fn a_reidentified_bundle_with_non_canonical_parameters_fails_verification() {
    type Edit = Box<dyn FnOnce(&mut Manifest)>;
    let cases: Vec<(&str, Edit)> = vec![
        (
            "an option at its default",
            Box::new(|m| {
                m.params
                    .insert("opt.stall_threshold_ms".into(), "250.0".into());
            }),
        ),
        (
            "an option not in its text form",
            Box::new(|m| {
                m.params
                    .insert("opt.stall_threshold_ms".into(), "5e2".into());
            }),
        ),
        (
            "an unknown key",
            Box::new(|m| {
                m.params.insert("zzz".into(), "1".into());
            }),
        ),
        (
            "an unknown arm",
            Box::new(|m| {
                m.params
                    .insert("arms".into(), "control,researcher,treatment".into());
            }),
        ),
        (
            "unsorted arms",
            Box::new(|m| {
                m.params.insert("arms".into(), "treatment,control".into());
            }),
        ),
        (
            "a malformed parameter name",
            Box::new(|m| {
                m.params.insert("vary.Bad Name".into(), "5".into());
            }),
        ),
    ];
    for (i, (what, edit)) in cases.into_iter().enumerate() {
        let runs = tempfile::tempdir().unwrap();
        let w = write(runs.path(), 100 + u64::try_from(i).unwrap());
        let moved = reidentify(&w.dir, edit);
        assert!(bundle::verify(&moved).is_err(), "{what}");
    }
    // The control: a canonical edit, re-identified, verifies under its new run_id.
    let runs = tempfile::tempdir().unwrap();
    let w = write(runs.path(), 120);
    let moved = reidentify(&w.dir, |m| {
        m.params
            .insert("opt.stall_threshold_ms".into(), "500.0".into());
    });
    assert!(bundle::verify(&moved).is_ok());
}

/// Cites: TRC-22, TRC-23
#[test]
fn only_the_layout_of_trc_22_may_be_listed() {
    for (path, allowed) in [
        ("verdict.json", false),
        ("views/anything.bin", false),
        ("views/turn.parquet", true),
        ("sidecar/provider/raw.json", true),
        ("../escape", false),
        ("logs/x", false),
        ("manifest.json", false),
    ] {
        let runs = tempfile::tempdir().unwrap();
        let w = write(runs.path(), 200);
        let file = w.dir.join(path);
        let inside = file.starts_with(&w.dir) && !path.contains("..");
        if inside && path != "manifest.json" {
            std::fs::create_dir_all(file.parent().unwrap()).unwrap();
            std::fs::write(&file, b"x").unwrap();
        }
        forge(&w.dir, |m| {
            m.files.insert(path.into(), Digest::of(b"x").to_hex());
        });
        assert_eq!(bundle::verify(&w.dir).is_ok(), allowed, "{path}");
    }
}

/// Cites: TRC-19, TRC-23
#[test]
fn verification_rechecks_the_resources_against_the_manifest() {
    let inv = acn_trace::schema::inventory().unwrap();
    type TraceEdit = Box<dyn Fn(&mut Trace)>;
    let edits: Vec<TraceEdit> = vec![
        Box::new(|t| {
            t.resources[0].attrs.insert(
                "acn.engine_hash".into(),
                AttrValue::String(Digest::of(b"other").to_hex()),
            );
        }),
        Box::new(|t| {
            t.resources[0].attrs.remove("service.name");
        }),
        Box::new(|t| {
            t.resources[0]
                .attrs
                .insert("service.name".into(), AttrValue::String("acn-gen".into()));
        }),
    ];
    for edit in edits {
        let runs = tempfile::tempdir().unwrap();
        let w = write(runs.path(), 300);
        let mut t = session(w.run_id.to_hex().as_str(), 300);
        edit(&mut t);
        let res = w.dir.join("resources.parquet");
        std::fs::remove_file(&res).unwrap();
        let [_, _, _, resources] = acn_trace::parquet_io::batches(&inv, &t).unwrap();
        acn_trace::parquet_io::write_batch(&res, &resources).unwrap();
        // The forger updates the listed hash too; only the content check is left.
        let h = acn_trace::identity::file_hash(&res).unwrap().to_hex();
        forge(&w.dir, |m| {
            m.files.insert("resources.parquet".into(), h.clone());
        });
        assert!(bundle::verify(&w.dir).is_err());
    }
}

/// Cites: CON-30, TRC-22
#[test]
fn run_seeds_must_fit_the_int64_seed_attribute() {
    let runs = tempfile::tempdir().unwrap();
    let pf = preflight(RunHypothesis::None);
    let top = i64::MAX.cast_unsigned();
    let b = Bundle::create(runs.path(), &pf, &build(), spec(top)).unwrap();
    let t = session(b.run_id(), top);
    let w = b.finish(&t).unwrap();
    assert_eq!(manifest(&w.dir).1.seed, "9223372036854775807");
    assert!(bundle::verify(&w.dir).is_ok());
    for seed in [top + 1, u64::MAX] {
        assert!(Bundle::create(runs.path(), &pf, &build(), spec(seed)).is_err());
    }
    assert!(
        fixture::session(&FixtureRun {
            run_id: "r".into(),
            seed: u64::MAX,
            replicate: 0,
            engine_hash: Digest::of(ENGINE),
            build_hash: Digest::of(b"b"),
        })
        .is_err(),
        "never clamped"
    );
    forge(&w.dir, |m| m.seed = u64::MAX.to_string());
    assert!(bundle::verify(&w.dir).is_err());
}
