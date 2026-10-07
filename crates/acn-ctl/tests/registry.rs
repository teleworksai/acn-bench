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
    // CON-27(d): the first vector of a preimage carries a float.
    let mut q = request("7");
    q.opt.stall_threshold_ms = Some(2000.0);
    let r = Resolved {
        request: q,
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
            r#""model":"mock-auto","opt":{"stall_threshold_ms":2000.0},"replicates":1,"seed":"7","vary":{},"workload":"w.toml"}}"#,
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
const KAV: &str = "bfc5ccd88f792389eed70ed190617fdc41d5936c729f64b36a3d0ff7631207af";

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
    assert_eq!(out.seq, 3);
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

/// Whether `cond` holds within a minute, looking every 10 ms. A channel that
/// never receives does the waiting: `std::thread::sleep` is banned (CON-5).
fn wait(cond: impl Fn() -> bool) -> bool {
    let (_tx, rx) = std::sync::mpsc::channel::<()>();
    for _ in 0..6000 {
        if cond() {
            return true;
        }
        let _ = rx.recv_timeout(std::time::Duration::from_millis(10));
    }
    cond()
}

fn status_path(dir: &Path, id: &str) -> std::path::PathBuf {
    dir.join("runs/ctl/requests").join(id).join("status.json")
}

fn set_state(dir: &Path, id: &str, state: &str) {
    let sp = status_path(dir, id);
    let mut st: serde_json::Value = serde_json::from_slice(&std::fs::read(&sp).unwrap()).unwrap();
    st["state"] = state.into();
    std::fs::write(&sp, serde_json::to_vec(&st).unwrap()).unwrap();
}

/// Cites: CTL-11
#[test]
fn a_restart_requeues_in_seq_order_and_handles_broken_requests() {
    let (dir, ctl) = workspace();
    // Ids in submission order, whatever their hex order.
    let ids: Vec<String> = ["5", "1", "9", "3"]
        .iter()
        .map(|s| ctl.submit(&request(s)).unwrap().request_id)
        .collect();
    let mut hex = ids.clone();
    hex.sort();
    assert_ne!(hex, ids, "pick seeds whose ids are not in submission order");
    // A request whose status was never written: queued last.
    std::fs::remove_file(status_path(dir.path(), &ids[1])).unwrap();
    // A directory with no request in it: removed.
    let stray = dir.path().join("runs/ctl/requests").join("ef".repeat(32));
    std::fs::create_dir_all(&stray).unwrap();
    // A request whose bytes are not its id's: failed, not run.
    let bad = &ids[2];
    std::fs::write(
        dir.path()
            .join("runs/ctl/requests")
            .join(bad)
            .join("request.json"),
        b"{}",
    )
    .unwrap();
    drop(ctl);
    let ctl = open(dir.path());
    assert!(!stray.exists());
    let ran: Vec<String> = std::iter::from_fn(|| ctl.run_next())
        .map(|o| o.request_id)
        .collect();
    assert_eq!(ran, [ids[0].clone(), ids[3].clone(), ids[1].clone()]);
    let st: serde_json::Value =
        serde_json::from_slice(&std::fs::read(status_path(dir.path(), bad)).unwrap()).unwrap();
    assert_eq!(st["state"], "failed");
    assert_eq!(st["code"], "internal");
}

/// Cites: CTL-11
#[test]
fn a_restart_keeps_a_bundle_that_verifies() {
    let (dir, ctl) = workspace();
    let a = ctl.submit(&request("7")).unwrap().request_id;
    let run_id = ctl.run_next().unwrap().status.run_id.unwrap();
    set_state(dir.path(), &a, "running");
    drop(ctl);
    let ctl = open(dir.path());
    let (_, st) = ctl.get(&a).unwrap();
    assert_eq!(st.code.as_deref(), Some("interrupted"));
    assert!(
        dir.path()
            .join("runs")
            .join(&run_id)
            .join("manifest.json")
            .exists()
    );
    // A retry adopts it.
    let mut r = request("7");
    r.retry = true;
    ctl.submit(&r).unwrap();
    let out = ctl.run_next().unwrap().status;
    assert_eq!((out.state, out.reused), (State::Done, Some(true)));
}

/// Cites: CTL-12, CTL-13
#[test]
fn the_worker_runs_one_at_a_time_refuses_a_retry_while_running_and_stops() {
    use std::sync::atomic::{AtomicUsize, Ordering};
    use std::sync::{Arc, Mutex};
    let (dir, _) = workspace();
    let now = Arc::new(AtomicUsize::new(0));
    let most = Arc::new(AtomicUsize::new(0));
    let gate = Arc::new(Mutex::new(()));
    let (n, m, g) = (now.clone(), most.clone(), gate.clone());
    let ctl = open(dir.path()).with_before_run(Arc::new(move || {
        let k = n.fetch_add(1, Ordering::SeqCst) + 1;
        m.fetch_max(k, Ordering::SeqCst);
        drop(g.lock().unwrap());
        n.fetch_sub(1, Ordering::SeqCst);
    }));
    let held = gate.lock().unwrap();
    let a = ctl.submit(&request("1")).unwrap().request_id;
    ctl.submit(&request("2")).unwrap();
    // Two workers on one registry: runs still never overlap.
    let w1 = std::thread::spawn({
        let c = ctl.clone();
        move || c.work()
    });
    let w2 = std::thread::spawn({
        let c = ctl.clone();
        move || c.work()
    });
    // While the first runs, a retry of it is refused.
    assert!(
        wait(|| ctl.get(&a).unwrap().1.state == State::Running),
        "the first request never started"
    );
    let mut retry = request("1");
    retry.retry = true;
    assert_eq!(ctl.submit(&retry).unwrap_err().status, 409);
    drop(held);
    assert!(
        wait(|| ctl.list().unwrap().iter().all(|(_, s)| *s == State::Done)),
        "{:?}",
        ctl.list().unwrap()
    );
    ctl.stop();
    w1.join().unwrap();
    w2.join().unwrap();
    assert_eq!(most.load(Ordering::SeqCst), 1, "two runs overlapped");
    // After stopping, submissions are refused.
    assert_eq!(ctl.submit(&request("3")).unwrap_err().status, 503);
}

/// Cites: CTL-12
#[test]
fn a_run_that_panics_fails_its_request_and_nothing_else() {
    let (dir, _) = workspace();
    let ctl = open(dir.path()).with_before_run(std::sync::Arc::new(|| panic!("boom")));
    ctl.submit(&request("7")).unwrap();
    let out = ctl.run_next().unwrap().status;
    assert_eq!(out.state, State::Failed);
    assert_eq!(out.code.as_deref(), Some("internal"));
    // The next request still runs on a registry without the hook.
    drop(ctl);
    let ctl = open(dir.path());
    ctl.submit(&request("8")).unwrap();
    assert_eq!(ctl.run_next().unwrap().status.state, State::Done);
}

/// Cites: CTL-10, CTL-30
#[test]
fn a_named_endpoint_resolves_to_its_url_and_must_match_the_backend() {
    let (dir, ctl) = workspace();
    let eps = dir.path().join("runs/ctl/endpoints");
    std::fs::create_dir_all(&eps).unwrap();
    std::fs::write(
        eps.join("mock.json"),
        r#"{"backend":"mockllm","url":"acn-mock://loopback"}"#,
    )
    .unwrap();
    std::fs::write(
        eps.join("oai.json"),
        r#"{"backend":"openai","url":"https://api.example.com"}"#,
    )
    .unwrap();
    let mut r = request("7");
    r.mode = "live".into();
    r.opt.endpoint_name = Some("mock".into());
    let id = ctl.submit(&r).unwrap().request_id;
    let (resolved, _) = ctl.get(&id).unwrap();
    assert_eq!(
        resolved.endpoint_url.as_deref(),
        Some("acn-mock://loopback")
    );
    r.opt.endpoint_name = Some("oai".into());
    assert_eq!(ctl.submit(&r).unwrap_err().code, "backend_mismatch");
    r.opt.endpoint_name = Some("nope".into());
    let e = ctl.submit(&r).unwrap_err();
    assert_eq!((e.status, e.code), (404, "unknown_endpoint"));
    // A generator runs on the mock: a provider's endpoint is a mismatch.
    std::fs::write(dir.path().join("s.toml"), SHEET).unwrap();
    let mut g = generator("7");
    g.opt.endpoint_name = Some("oai".into());
    assert_eq!(ctl.submit(&g).unwrap_err().code, "backend_mismatch");
}

const SHEET: &str = r#"schema_version = 1
placeholder = true
doc = "a small sheet"
model = "mock-agentic"
sessions = 1
system_tokens = 20
summary_instruction_tokens = 4
summary_max_tokens = 8
compact_at_tokens = 0
session_start_ns = { const = 0 }
turns_per_session = { const = 1 }
think_time_ns = { const = 0 }
chain_length = { const = 1 }
fanout_width = { const = 0 }
user_tokens = { const = 5 }
answer_tokens = { const = 8 }
tool_class = { weighted = [["file", 1]] }

[tool_result_tokens]
file = { const = 10 }

[tool_duration_ns]
file = { const = 1_000_000 }
"#;

fn generator(seed: &str) -> Submit {
    Submit {
        kind: Kind::Generator,
        workload: None,
        backend: None,
        model: None,
        sheet: Some("s.toml".into()),
        ..request(seed)
    }
}

/// Cites: CTL-10, CTL-13
#[test]
fn a_generator_request_and_a_stored_scenario_run_through_the_registry() {
    let (dir, ctl) = workspace();
    std::fs::write(dir.path().join("s.toml"), SHEET).unwrap();
    let out = {
        ctl.submit(&generator("7")).unwrap();
        ctl.run_next().unwrap().status
    };
    assert_eq!(out.state, State::Done, "{out:?}");
    // A stored scenario, named by the hash of its bytes.
    let toml = "schema_version = 1\nname = \"p\"\n\n[[link]]\nname = \"p\"\ndirection = \"up\"\n\n[[link]]\nname = \"p\"\ndirection = \"down\"\n";
    let hash = blake3::hash(toml.as_bytes()).to_hex().to_string();
    let sd = dir.path().join("runs/ctl/scenarios").join(&hash);
    std::fs::create_dir_all(&sd).unwrap();
    std::fs::write(sd.join("p.toml"), toml).unwrap();
    let mut r = request("7");
    r.scenario = Some(acn_ctl::ScenarioRef::Hash(acn_ctl::request::ByHash {
        hash: hash.clone(),
    }));
    ctl.submit(&r).unwrap();
    let out = ctl.run_next().unwrap().status;
    assert_eq!(out.state, State::Done, "{out:?}");
    let manifest: serde_json::Value = serde_json::from_slice(
        &std::fs::read(
            dir.path()
                .join("runs")
                .join(out.run_id.unwrap())
                .join("manifest.json"),
        )
        .unwrap(),
    )
    .unwrap();
    assert_eq!(manifest["scenario_hash"], hash);
    r.scenario = Some(acn_ctl::ScenarioRef::Hash(acn_ctl::request::ByHash {
        hash: "0".repeat(64),
    }));
    let e = ctl.submit(&r).unwrap_err();
    assert_eq!((e.status, e.code), (404, "unknown_scenario"));
}

/// Cites: CTL-10
#[test]
fn a_seed_has_one_text() {
    let (_dir, ctl) = workspace();
    assert_eq!(ctl.submit(&request("007")).unwrap_err().code, "bad_request");
    let mut r = request("7");
    r.opt.stall_threshold_ms = Some(-1.0);
    assert_eq!(ctl.submit(&r).unwrap_err().code, "bad_request");
}
