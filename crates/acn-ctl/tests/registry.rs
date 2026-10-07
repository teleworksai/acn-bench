//! The registry and its worker (SPEC 070 CTL-10 to CTL-13), through the
//! `Ctl` facade: no HTTP.

#![allow(clippy::unwrap_used, clippy::expect_used, clippy::panic)] // CON-19: tests are exempt

use std::collections::BTreeMap;
use std::path::Path;

use acn_ctl::request::{ByPath, ReqOpts};
use acn_ctl::resolve::Resolved;
use acn_ctl::{Ctl, CtlConfig, Kind, State, Submit};
use acn_trace::identity::{BuildParts, Digest};

fn build() -> acn_trace::identity::BuildInfo {
    BuildParts {
        cargo_lock: Digest::of(b"lock"),
        rust_toolchain: Digest::of(b"toolchain"),
        cargo_config: Digest::of(b"config"),
        source_hash: Digest::of(b"build"),
        target: "test",
        profile: "debug",
        features: "",
        rustflags: "",
    }
    .info()
    .unwrap()
}

/// A workspace with the smoke workload at `w.toml`, and its control plane.
fn workspace() -> (tempfile::TempDir, Ctl) {
    let dir = tempfile::tempdir().unwrap();
    let smoke = std::fs::read_to_string(concat!(
        env!("CARGO_MANIFEST_DIR"),
        "/../../workloads/harness-smoke.toml"
    ))
    .unwrap();
    std::fs::write(dir.path().join("w.toml"), smoke).unwrap();
    let ctl = open(dir.path());
    (dir, ctl)
}

fn open(root: &Path) -> Ctl {
    Ctl::open(CtlConfig {
        root: root.to_path_buf(),
        runs_dir: "runs".into(),
        engine_hash: Digest::of(b"engine"),
        build: build(),
        profiles: None,
    })
    .unwrap()
}

fn request(seed: &str) -> Submit {
    Submit {
        kind: Kind::Harness,
        workload: Some("w.toml".into()),
        backend: Some("mockllm".into()),
        model: Some("mock-auto".into()),
        sheet: None,
        mode: "sim".into(),
        arm: "treatment".into(),
        replicates: 1,
        vary: BTreeMap::new(),
        hypothesis: None,
        seed: Some(seed.into()),
        opt: ReqOpts::default(),
        scenario: None,
        retry: false,
    }
}

/// Cites: CTL-11
#[test]
fn a_request_id_is_the_hash_of_its_context_and_canonical_bytes() {
    let r = Resolved {
        request: request("7"),
        hashes: BTreeMap::from([("w.toml".into(), "ab".repeat(32))]),
        endpoint_url: None,
    };
    // Keys sorted, no whitespace, one newline (CTL-11).
    let text = r.text().unwrap();
    assert_eq!(
        text,
        concat!(
            r#"{"hashes":{"w.toml":"abababababababababababababababababababababababababababababababab"},"#,
            r#""request":{"arm":"treatment","backend":"mockllm","kind":"harness","mode":"sim","#,
            r#""model":"mock-auto","opt":{},"replicates":1,"seed":"7","vary":{},"workload":"w.toml"}}"#,
            "\n"
        )
    );
    let mut h = blake3::Hasher::new();
    h.update(b"acn-bench/ctl_request/v1\0");
    h.update(text.as_bytes());
    assert_eq!(r.request_id().unwrap(), h.finalize().to_hex().to_string());
    assert_eq!(r.request_id().unwrap(), KAV);
}

/// The known-answer `request_id` of the request above (CON-27(d)).
const KAV: &str = "bbfa01752a676db886444e883e51cbe6c423390e157ca6dfcd606ae87820b4a0";

/// Cites: CTL-13, CTL-12
#[test]
fn a_request_runs_once_and_a_resubmission_returns_its_status() {
    let (_dir, ctl) = workspace();
    let a = ctl.submit(&request("7")).unwrap();
    assert!(a.new);
    assert_eq!(a.state, State::Queued);
    let again = ctl.submit(&request("7")).unwrap();
    assert!(!again.new);
    assert_eq!(again.request_id, a.request_id);
    let out = ctl.run_next().unwrap();
    assert_eq!(out.status.state, State::Done, "{:?}", out.status);
    assert_eq!(out.status.reused, Some(false));
    // Nothing more queued: the resubmission ran nothing.
    assert!(ctl.run_next().is_none());
    let done = ctl.submit(&request("7")).unwrap();
    assert_eq!(done.state, State::Done);
    assert!(ctl.run_next().is_none());
    assert_eq!(ctl.run_ids(), [out.status.run_id.unwrap()]);
}

/// Cites: CTL-11, CTL-12
#[test]
fn an_edited_input_is_a_new_request_and_an_edit_after_submission_fails() {
    let (dir, ctl) = workspace();
    let a = ctl.submit(&request("7")).unwrap();
    let w = dir.path().join("w.toml");
    let text = std::fs::read_to_string(&w).unwrap();
    std::fs::write(&w, format!("{text}\n# edited\n")).unwrap();
    let b = ctl.submit(&request("7")).unwrap();
    assert_ne!(a.request_id, b.request_id);
    // The first request's input has changed since it was submitted.
    let out = ctl.run_next().unwrap();
    assert_eq!(out.request_id, a.request_id);
    assert_eq!(out.status.state, State::Failed);
    assert_eq!(out.status.code.as_deref(), Some("input_changed"));
    let out = ctl.run_next().unwrap();
    assert_eq!(out.status.state, State::Done);
}

/// Cites: CTL-12
#[test]
fn requests_run_in_submission_order() {
    let (_dir, ctl) = workspace();
    let ids: Vec<String> = ["3", "1", "2"]
        .iter()
        .map(|s| ctl.submit(&request(s)).unwrap().request_id)
        .collect();
    let ran: Vec<String> = std::iter::from_fn(|| ctl.run_next())
        .map(|o| o.request_id)
        .collect();
    assert_eq!(ran, ids);
    let seqs: Vec<u64> = ids.iter().map(|i| ctl.get(i).unwrap().1.seq).collect();
    assert_eq!(seqs, [0, 1, 2]);
}

/// Cites: CTL-11
#[test]
fn a_restart_marks_an_unfinished_run_interrupted_and_requeues_the_rest() {
    let (dir, ctl) = workspace();
    let a = ctl.submit(&request("7")).unwrap().request_id;
    let b = ctl.submit(&request("8")).unwrap().request_id;
    // A was running when the server stopped, half its bundle written.
    let run_id = "cd".repeat(32);
    let partial = dir.path().join("runs").join(&run_id);
    std::fs::create_dir_all(&partial).unwrap();
    std::fs::write(partial.join("spans.parquet"), b"half").unwrap();
    let sp = dir
        .path()
        .join("runs/ctl/requests")
        .join(&a)
        .join("status.json");
    let mut st: serde_json::Value = serde_json::from_slice(&std::fs::read(&sp).unwrap()).unwrap();
    st["state"] = "running".into();
    st["run_id"] = run_id.clone().into();
    std::fs::write(&sp, serde_json::to_vec(&st).unwrap()).unwrap();
    drop(ctl);
    let ctl = open(dir.path());
    let (_, sa) = ctl.get(&a).unwrap();
    assert_eq!(sa.state, State::Failed);
    assert_eq!(sa.code.as_deref(), Some("interrupted"));
    assert!(!partial.exists(), "the unfinished bundle is removed");
    // B is queued again, and runs.
    let out = ctl.run_next().unwrap();
    assert_eq!(out.request_id, b);
    assert_eq!(out.status.state, State::Done);
    assert!(ctl.run_next().is_none());
}

/// Cites: CTL-13
#[test]
fn an_existing_bundle_is_adopted_and_a_failed_request_reruns_only_on_retry() {
    let (dir, ctl) = workspace();
    let a = ctl.submit(&request("7")).unwrap().request_id;
    let done = ctl.run_next().unwrap().status;
    let run_id = done.run_id.clone().unwrap();
    // The same run, asked with an option at its default: another request,
    // one run_id, the bundle adopted.
    let mut b = request("7");
    b.opt.max_retries = Some(3);
    let bid = ctl.submit(&b).unwrap().request_id;
    assert_ne!(bid, a);
    let out = ctl.run_next().unwrap().status;
    assert_eq!(out.state, State::Done);
    assert_eq!(out.reused, Some(true));
    assert_eq!(out.run_id.as_deref(), Some(run_id.as_str()));
    assert_eq!(out.bundle_digest, done.bundle_digest);
    // A bundle that no longer verifies is not adopted.
    let bundle = dir.path().join("runs").join(&run_id);
    std::fs::write(bundle.join("spans.parquet"), b"broken").unwrap();
    let mut c = request("7");
    c.opt.retry_base_ms = Some(500);
    let cid = ctl.submit(&c).unwrap().request_id;
    let out = ctl.run_next().unwrap().status;
    assert_eq!(out.state, State::Failed);
    assert_eq!(out.code.as_deref(), Some("bundle_invalid"));
    // Resubmitting a failed request runs nothing; with `retry` it runs.
    assert_eq!(ctl.submit(&c).unwrap().state, State::Failed);
    assert!(ctl.run_next().is_none());
    std::fs::remove_dir_all(&bundle).unwrap();
    c.retry = true;
    let again = ctl.submit(&c).unwrap();
    assert_eq!(again.request_id, cid);
    assert_eq!(again.state, State::Queued);
    let out = ctl.run_next().unwrap().status;
    assert_eq!(out.state, State::Done, "{out:?}");
    assert_eq!(out.reused, Some(false));
    assert!(out.seq > 2);
}

/// Cites: CTL-10, CTL-1
#[test]
fn paths_outside_the_workspace_bad_fields_and_protected_runs_dirs_are_refused() {
    let (dir, ctl) = workspace();
    let refused = |r: &Submit, code: &str| {
        let e = ctl.submit(r).unwrap_err();
        assert_eq!(e.code, code, "{e}");
    };
    let mut r = request("7");
    r.workload = Some("../w.toml".into());
    refused(&r, "path_refused");
    r.workload = Some("/etc/hosts".into());
    refused(&r, "path_refused");
    // A link out of the workspace.
    let outside = tempfile::tempdir().unwrap();
    std::fs::write(outside.path().join("x.toml"), "x").unwrap();
    #[cfg(unix)]
    {
        std::os::unix::fs::symlink(outside.path().join("x.toml"), dir.path().join("l.toml"))
            .unwrap();
        r.workload = Some("l.toml".into());
        refused(&r, "path_refused");
    }
    let mut r = request("7");
    r.hypothesis = Some("w.toml".into());
    refused(&r, "bad_request");
    let mut r = request("7");
    r.seed = Some("99999999999999999999".into());
    refused(&r, "bad_request");
    let mut r = request("7");
    r.scenario = Some(acn_ctl::ScenarioRef::Path(ByPath {
        path: "missing.toml".into(),
    }));
    refused(&r, "path_refused");
    // Unknown fields are refused when parsed (CTL-2).
    let e = Submit::parse(br#"{"kind":"harness","mode":"sim","arm":"treatment","replicates":1,"seed":"7","keep_content":true}"#).unwrap_err();
    assert_eq!(e.code, "bad_request");
    // A runs directory inside a protected path (LOOP-20).
    for bad in ["hypotheses", "specs/x", "scenarios/measured", "../runs"] {
        let e = Ctl::open(CtlConfig {
            root: dir.path().to_path_buf(),
            runs_dir: bad.into(),
            engine_hash: Digest::of(b"engine"),
            build: build(),
            profiles: None,
        })
        .err()
        .unwrap();
        assert_eq!(e.code, "runs_dir_refused", "{bad}");
    }
}
