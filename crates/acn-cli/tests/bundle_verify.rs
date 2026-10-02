//! `acn bundle verify` (TRC-23) under the CLI contract (CON-8).

#![allow(clippy::unwrap_used, clippy::expect_used, clippy::panic)] // CON-19: tests are exempt

use std::collections::BTreeMap;
use std::path::Path;
use std::process::Command;

use acn_trace::bundle::{Bundle, HypothesisRef, RunSpec};
use acn_trace::env::{self, RunHypothesis};
use acn_trace::fixture::{self, FixtureRun};
use acn_trace::identity::{BuildParts, Digest, HypStatus, Mode, RunParams};

fn acn(args: &[&str]) -> (Option<i32>, serde_json::Value) {
    let out = Command::new(env!("CARGO_BIN_EXE_acn"))
        .args(args)
        .output()
        .unwrap();
    let json = serde_json::from_slice(&out.stdout).unwrap();
    (out.status.code(), json)
}

fn bundle(runs: &Path) -> (std::path::PathBuf, Digest, Digest) {
    let build = BuildParts {
        cargo_lock: Digest::of(b"l"),
        rust_toolchain: Digest::of(b"t"),
        cargo_config: Digest::of(b"c"),
        source_hash: Digest::of(b"s"),
        target: "t",
        profile: "debug",
        features: "",
        rustflags: "",
    }
    .info()
    .unwrap();
    let outside = tempfile::tempdir().unwrap();
    let pf = env::preflight(outside.path(), Digest::of(b"e"), RunHypothesis::None).unwrap();
    let b = Bundle::create(
        runs,
        &pf,
        &build,
        RunSpec {
            seed: 3,
            mode: Mode::Sim,
            scenario_hash: Digest::of(b"scenario"),
            workload_hash: Digest::of(b"workload"),
            hypothesis: HypothesisRef::none(),
            params: RunParams {
                backend: "mockllm".into(),
                model: "m".into(),
                hyp_status: HypStatus::Candidate,
                arms: vec!["control".into()],
                replicates: 1,
                vary: BTreeMap::new(),
                opts: BTreeMap::new(),
            },
            endpoint_host: None,
            execution_order: None,
            started_at: None,
        },
    )
    .unwrap();
    let t = fixture::session(&FixtureRun {
        run_id: b.run_id().into(),
        seed: 3,
        replicate: 0,
        engine_hash: Digest::of(b"e"),
        build_hash: Digest::from_hex(&build.build_hash).unwrap(),
    })
    .unwrap();
    let w = b.finish(&t).unwrap();
    (w.dir, w.run_id, w.bundle_digest)
}

/// Cites: TRC-23, CON-8
#[test]
fn verify_prints_the_run_id_and_bundle_digest() {
    let runs = tempfile::tempdir().unwrap();
    let (dir, run_id, digest) = bundle(runs.path());
    let (code, json) = acn(&["bundle", "verify", dir.to_str().unwrap()]);
    assert_eq!(code, Some(0), "{json}");
    assert_eq!(json["ok"], true);
    assert_eq!(json["run_id"], run_id.to_hex().as_str());
    assert_eq!(json["bundle_digest"], digest.to_hex().as_str());
}

/// Cites: TRC-23, CON-8
#[test]
fn a_tampered_bundle_is_a_json_error_with_exit_one() {
    let runs = tempfile::tempdir().unwrap();
    let (dir, _, _) = bundle(runs.path());
    std::fs::write(dir.join("unlisted.bin"), b"x").unwrap();
    let (code, json) = acn(&["bundle", "verify", dir.to_str().unwrap()]);
    assert_eq!(code, Some(1));
    assert_eq!(json["ok"], false);
    assert!(json["error"].as_str().unwrap().contains("unlisted.bin"));
    let (code, json) = acn(&["bundle", "verify", "/nonexistent/bundle"]);
    assert_eq!((code, &json["ok"]), (Some(1), &serde_json::json!(false)));
}

/// Cites: TRC-35, CON-8
#[test]
fn verify_views_recomputes_and_reports_it() {
    let runs = tempfile::tempdir().unwrap();
    let (dir, run_id, _) = bundle(runs.path());
    let (code, json) = acn(&["bundle", "verify", "--views", dir.to_str().unwrap()]);
    assert_eq!(code, Some(0), "{json}");
    assert_eq!(json["views_recomputed"], true);
    assert_eq!(json["run_id"], run_id.to_hex().as_str());
}

/// A one-shot OTLP/HTTP collector on localhost: it answers `status` to the first
/// request and hands back the body it received.
fn stub_collector(status: &'static str) -> (String, std::thread::JoinHandle<String>) {
    use std::io::{BufRead as _, BufReader, Read as _, Write as _};
    let listener = std::net::TcpListener::bind("127.0.0.1:0").unwrap();
    let addr = listener.local_addr().unwrap();
    let handle = std::thread::spawn(move || {
        let (stream, _) = listener.accept().unwrap();
        let mut reader = BufReader::new(stream.try_clone().unwrap());
        let mut length = 0usize;
        loop {
            let mut line = String::new();
            reader.read_line(&mut line).unwrap();
            if line == "\r\n" {
                break;
            }
            if let Some(v) = line.to_ascii_lowercase().strip_prefix("content-length:") {
                length = v.trim().parse().unwrap();
            }
        }
        let mut body = vec![0u8; length];
        reader.read_exact(&mut body).unwrap();
        let mut out = stream;
        write!(
            out,
            "HTTP/1.1 {status}\r\ncontent-length: 2\r\nconnection: close\r\n\r\n{{}}"
        )
        .unwrap();
        String::from_utf8(body).unwrap()
    });
    (format!("http://{addr}/v1/traces"), handle)
}

/// Cites: TRC-28, CON-8
#[test]
fn export_replays_a_bundle_to_an_otlp_collector_and_to_a_file() {
    let runs = tempfile::tempdir().unwrap();
    let (dir, run_id, _) = bundle(runs.path());
    let (endpoint, server) = stub_collector("200 OK");
    let (code, json) = acn(&[
        "bundle",
        "export",
        dir.to_str().unwrap(),
        "--otlp",
        &endpoint,
    ]);
    assert_eq!(code, Some(0), "{json}");
    assert_eq!(json["run_id"], run_id.to_hex().as_str());
    let posted = server.join().unwrap();
    let doc: serde_json::Value = serde_json::from_str(&posted).unwrap();
    assert!(doc["resourceSpans"].is_array());

    let out = runs.path().join("export.json");
    let (code, json) = acn(&[
        "bundle",
        "export",
        dir.to_str().unwrap(),
        "--otlp-json",
        out.to_str().unwrap(),
    ]);
    assert_eq!(code, Some(0), "{json}");
    assert_eq!(
        std::fs::read_to_string(&out).unwrap(),
        posted,
        "one document, either way"
    );
    // A file is never replaced, and a failing collector is a JSON error.
    let (code, _) = acn(&[
        "bundle",
        "export",
        dir.to_str().unwrap(),
        "--otlp-json",
        out.to_str().unwrap(),
    ]);
    assert_eq!(code, Some(1));
    let (endpoint, server) = stub_collector("503 Service Unavailable");
    let (code, json) = acn(&[
        "bundle",
        "export",
        dir.to_str().unwrap(),
        "--otlp",
        &endpoint,
    ]);
    assert_eq!((code, &json["ok"]), (Some(1), &serde_json::json!(false)));
    let _ = server.join();
}

/// Cites: TRC-28, TRC-26, CON-8
#[test]
fn import_writes_tables_and_views_from_otlp_json() {
    let runs = tempfile::tempdir().unwrap();
    let (dir, _, _) = bundle(runs.path());
    let doc = runs.path().join("doc.json");
    let (code, _) = acn(&[
        "bundle",
        "export",
        dir.to_str().unwrap(),
        "--otlp-json",
        doc.to_str().unwrap(),
    ]);
    assert_eq!(code, Some(0));
    let out = runs.path().join("imported");
    let (code, json) = acn(&[
        "bundle",
        "import",
        "--otlp-json",
        doc.to_str().unwrap(),
        "--out",
        out.to_str().unwrap(),
    ]);
    assert_eq!(code, Some(0), "{json}");
    for f in ["spans.parquet", "views/turn.parquet", "views/call.parquet"] {
        assert_eq!(
            std::fs::read(out.join(f)).unwrap(),
            std::fs::read(dir.join(f)).unwrap(),
            "{f} is identical to the bundle's"
        );
    }
    let (code, _) = acn(&[
        "bundle",
        "import",
        "--otlp-json",
        doc.to_str().unwrap(),
        "--out",
        out.to_str().unwrap(),
    ]);
    assert_eq!(code, Some(1), "an existing directory is never written into");
}
