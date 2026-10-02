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

/// What a stub collector saw of one request.
#[derive(Debug)]
struct Seen {
    request_line: String,
    headers: Vec<String>,
    body: String,
}

/// An OTLP/HTTP collector on localhost that serves `n` requests, answering each
/// with `status` (or never answering, for `None`), and sends what it saw. The test
/// waits for it with a deadline, so a CLI that never connects fails the test
/// instead of hanging it.
fn stub_collector(
    n: usize,
    status: Option<&'static str>,
) -> (String, std::sync::mpsc::Receiver<Vec<Seen>>) {
    use std::io::{BufRead as _, BufReader, Read as _, Write as _};
    let listener = std::net::TcpListener::bind("127.0.0.1:0").unwrap();
    let addr = listener.local_addr().unwrap();
    let (tx, rx) = std::sync::mpsc::channel();
    std::thread::spawn(move || {
        let mut seen = Vec::new();
        let mut held = Vec::new();
        for _ in 0..n {
            let (stream, _) = listener.accept().unwrap();
            let mut reader = BufReader::new(stream.try_clone().unwrap());
            let mut request_line = String::new();
            reader.read_line(&mut request_line).unwrap();
            let mut headers = Vec::new();
            let mut length = 0usize;
            loop {
                let mut line = String::new();
                reader.read_line(&mut line).unwrap();
                if line == "\r\n" {
                    break;
                }
                let lower = line.trim_end().to_ascii_lowercase();
                if let Some(v) = lower.strip_prefix("content-length:") {
                    length = v.trim().parse().unwrap();
                }
                headers.push(lower);
            }
            let mut body = vec![0u8; length];
            reader.read_exact(&mut body).unwrap();
            seen.push(Seen {
                request_line: request_line.trim_end().to_owned(),
                headers,
                body: String::from_utf8(body).unwrap(),
            });
            let mut out = stream;
            match status {
                Some(status) => write!(
                    out,
                    "HTTP/1.1 {status}\r\nlocation: http://{addr}/elsewhere\r\ncontent-length: 2\r\nconnection: close\r\n\r\n{{}}"
                )
                .unwrap(),
                None => held.push(out), // never answer
            }
        }
        let _ = tx.send(seen);
        drop(held);
    });
    (format!("http://{addr}/v1/traces"), rx)
}

fn wait(rx: &std::sync::mpsc::Receiver<Vec<Seen>>) -> Vec<Seen> {
    rx.recv_timeout(std::time::Duration::from_secs(20))
        .expect("the collector saw the expected requests")
}

/// Cites: TRC-28, CON-8
#[test]
fn export_posts_deterministic_chunks_an_otlp_collector_accepts() {
    let runs = tempfile::tempdir().unwrap();
    let (dir, run_id, _) = bundle(runs.path());
    let inv = acn_trace::schema::inventory().unwrap();
    let trace = acn_trace::parquet_io::read_trace(&dir, &inv).unwrap();
    let requests = trace.spans.len().div_ceil(3);
    let (endpoint, rx) = stub_collector(requests, Some("200 OK"));
    // Credentials in the endpoint never reach the output.
    let with_secret = endpoint.replace("http://", "http://user:s3cret@");
    let (code, json) = acn(&[
        "bundle",
        "export",
        dir.to_str().unwrap(),
        "--otlp",
        &with_secret,
        "--max-spans",
        "3",
    ]);
    assert_eq!(code, Some(0), "{json}");
    assert_eq!(json["run_id"], run_id.to_hex().as_str());
    assert_eq!(json["requests"], requests);
    assert!(!json.to_string().contains("s3cret"), "{json}");
    let seen = wait(&rx);
    let mut docs = Vec::new();
    for r in &seen {
        assert_eq!(r.request_line, "POST /v1/traces HTTP/1.1");
        assert!(
            r.headers
                .iter()
                .any(|h| h == "content-type: application/json"),
            "{:?}",
            r.headers
        );
        docs.push(serde_json::from_str::<serde_json::Value>(&r.body).unwrap());
    }
    assert_eq!(
        acn_trace::otlp::from_json_many(&docs).unwrap(),
        trace,
        "the chunks are the bundle"
    );
}

/// Cites: TRC-28, CON-8
#[test]
fn a_refusing_redirecting_or_silent_collector_is_a_json_error() {
    let runs = tempfile::tempdir().unwrap();
    let (dir, _, _) = bundle(runs.path());
    for (status, needle) in [
        (Some("503 Service Unavailable"), "503"),
        (Some("302 Found"), "302"),
        (None, ""),
    ] {
        let (endpoint, rx) = stub_collector(1, status);
        let (code, json) = acn(&[
            "bundle",
            "export",
            dir.to_str().unwrap(),
            "--otlp",
            &endpoint,
            "--timeout-secs",
            "1",
        ]);
        assert_eq!(
            (code, &json["ok"]),
            (Some(1), &serde_json::json!(false)),
            "{status:?}: {json}"
        );
        assert!(
            json["error"].as_str().unwrap().contains(needle),
            "{status:?}: {json}"
        );
        if status.is_some() {
            assert_eq!(wait(&rx).len(), 1, "one request, no redirect followed");
        }
    }
}

/// Cites: TRC-28, TRC-23, CON-8
#[test]
fn export_to_a_file_never_replaces_one_and_never_exports_a_tampered_bundle() {
    let runs = tempfile::tempdir().unwrap();
    let (dir, _, _) = bundle(runs.path());
    let out = runs.path().join("export.json");
    let (code, json) = acn(&[
        "bundle",
        "export",
        dir.to_str().unwrap(),
        "--otlp-json",
        out.to_str().unwrap(),
    ]);
    assert_eq!(code, Some(0), "{json}");
    let first = std::fs::read(&out).unwrap();
    std::fs::write(&out, b"mine").unwrap();
    let (code, _) = acn(&[
        "bundle",
        "export",
        dir.to_str().unwrap(),
        "--otlp-json",
        out.to_str().unwrap(),
    ]);
    assert_eq!(code, Some(1));
    assert_eq!(
        std::fs::read(&out).unwrap(),
        b"mine",
        "the existing file is untouched"
    );
    assert!(!runs.path().join(".export.json.partial").exists());
    // A tampered bundle is not exported.
    std::fs::write(dir.join("unlisted.bin"), b"x").unwrap();
    let other = runs.path().join("other.json");
    let (code, _) = acn(&[
        "bundle",
        "export",
        dir.to_str().unwrap(),
        "--otlp-json",
        other.to_str().unwrap(),
    ]);
    assert_eq!(code, Some(1));
    assert!(!other.exists());
    assert!(!first.is_empty());
}

/// Cites: TRC-28, TRC-26, CON-8
#[test]
fn import_merges_a_nodes_own_export_with_its_run_and_aligns_its_clock() {
    use serde_json::json;
    let runs = tempfile::tempdir().unwrap();
    let (dir, _, _) = bundle(runs.path());
    let inv = acn_trace::schema::inventory().unwrap();
    let run = acn_trace::parquet_io::read_trace(&dir, &inv).unwrap();
    let chat = run.spans.iter().find(|s| s.name == "chat").unwrap();
    let hex = |b: &[u8]| b.iter().map(|x| format!("{x:02x}")).collect::<String>();
    // The node's own export: its request span carries the call's traceparent and
    // the node's clock, 10 s ahead, centred on the call.
    let skew = 10_000_000_000i64;
    let mid = (chat.start_ns + chat.end_ns) / 2;
    let node = json!({ "resourceSpans": [{
        "resource": { "attributes": [{ "key": "service.name", "value": { "stringValue": "vllm" } }] },
        "scopeSpans": [{ "spans": [{
            "traceId": hex(&chat.trace_id), "spanId": "0102030405060708",
            "parentSpanId": hex(&chat.span_id), "name": "llm_request", "kind": 2,
            "startTimeUnixNano": (mid - 1000 + skew).to_string(),
            "endTimeUnixNano": (mid + 1000 + skew).to_string()
        }] }]
    }] });
    let doc = runs.path().join("node.json");
    std::fs::write(&doc, node.to_string()).unwrap();
    // On its own the node's export names a parent it does not hold.
    let alone = runs.path().join("alone");
    let (code, json) = acn(&[
        "bundle",
        "import",
        "--otlp-json",
        doc.to_str().unwrap(),
        "--out",
        alone.to_str().unwrap(),
    ]);
    assert_eq!(code, Some(1), "{json}");
    assert!(
        !alone.exists() && !runs.path().join(".alone.partial").exists(),
        "nothing left behind"
    );

    let out = runs.path().join("merged");
    let (code, json) = acn(&[
        "bundle",
        "import",
        "--otlp-json",
        doc.to_str().unwrap(),
        "--with",
        dir.to_str().unwrap(),
        "--out",
        out.to_str().unwrap(),
    ]);
    assert_eq!(code, Some(0), "{json}");
    assert_eq!(json["aligned_spans"], 1);
    let merged = acn_trace::parquet_io::read_trace(&out, &inv).unwrap();
    let n = merged
        .spans
        .iter()
        .find(|s| s.name == "llm_request")
        .unwrap();
    assert_eq!((n.start_ns, n.end_ns), (mid - 1000, mid + 1000));
    assert_eq!(
        n.attrs[acn_trace::ingest::CLOCK_OFFSET],
        acn_trace::model::AttrValue::Int(skew)
    );
    for v in [
        "views/session.parquet",
        "views/turn.parquet",
        "views/call.parquet",
        "views/tool.parquet",
        "views/link.parquet",
    ] {
        assert_eq!(
            std::fs::read(out.join(v)).unwrap(),
            std::fs::read(dir.join(v)).unwrap(),
            "{v}"
        );
    }
    // Bad JSON fails cleanly too.
    let bad = runs.path().join("bad.json");
    std::fs::write(&bad, "{ not json").unwrap();
    let out2 = runs.path().join("bad-out");
    let (code, _) = acn(&[
        "bundle",
        "import",
        "--otlp-json",
        bad.to_str().unwrap(),
        "--out",
        out2.to_str().unwrap(),
    ]);
    assert_eq!(code, Some(1));
    assert!(!out2.exists());
    let (code, _) = acn(&[
        "bundle",
        "import",
        "--otlp-json",
        doc.to_str().unwrap(),
        "--with",
        dir.to_str().unwrap(),
        "--out",
        out.to_str().unwrap(),
    ]);
    assert_eq!(code, Some(1), "an existing directory is never written into");
}
