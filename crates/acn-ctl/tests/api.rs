//! The API (SPEC 070 CTL-1 to CTL-3, CTL-20 to CTL-24, CTL-30) on an
//! in-process server, spoken to in raw HTTP/1.1 so that no client rewrites
//! the headers under test.

#![allow(clippy::unwrap_used, clippy::expect_used, clippy::panic)] // CON-19: tests are exempt

use std::io::{Read as _, Write as _};
use std::net::SocketAddr;
use std::path::Path;

use acn_ctl::CtlConfig;
use acn_ctl::registry::Ctl;
use acn_ctl::server::{Server, Summary};
use acn_trace::identity::{BuildParts, Digest};
use serde_json::{Value, json};

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

struct Running {
    addr: SocketAddr,
    dir: tempfile::TempDir,
    join: std::thread::JoinHandle<Summary>,
}

fn config(dir: &Path) -> CtlConfig {
    CtlConfig {
        root: dir.to_path_buf(),
        runs_dir: "runs".into(),
        engine_hash: Digest::of(b"engine"),
        build: build(),
        profiles: None,
    }
}

fn serve() -> Running {
    serve_with(|c| c)
}

/// A server whose registry `f` sets up (its hooks) before it binds.
fn serve_with(f: impl FnOnce(Ctl) -> Ctl) -> Running {
    let dir = tempfile::tempdir().unwrap();
    let smoke = std::fs::read_to_string(concat!(
        env!("CARGO_MANIFEST_DIR"),
        "/../../workloads/harness-smoke.toml"
    ))
    .unwrap();
    std::fs::write(dir.path().join("w.toml"), smoke).unwrap();
    let ctl = f(Ctl::open(config(dir.path())).unwrap());
    let s = Server::bind_with(ctl, 0).unwrap();
    let addr = s.addr();
    let join = std::thread::spawn(move || s.run());
    Running { addr, dir, join }
}

/// One request; its status and body.
fn raw(addr: SocketAddr, head: &str, body: &[u8]) -> (u16, Vec<u8>) {
    let mut s = std::net::TcpStream::connect(addr).unwrap();
    s.write_all(
        format!(
            "{head}Content-Length: {}\r\nConnection: close\r\n\r\n",
            body.len()
        )
        .as_bytes(),
    )
    .unwrap();
    s.write_all(body).unwrap();
    let mut out = Vec::new();
    s.read_to_end(&mut out).unwrap();
    let split = out.windows(4).position(|w| w == b"\r\n\r\n").unwrap();
    let head = String::from_utf8_lossy(&out[..split]).into_owned();
    let status: u16 = head.split(' ').nth(1).unwrap().parse().unwrap();
    (status, out[split + 4..].to_vec())
}

fn call(
    addr: SocketAddr,
    method: &str,
    path: &str,
    ct: Option<&str>,
    body: &[u8],
) -> (u16, Vec<u8>) {
    let ct = ct
        .map(|c| format!("Content-Type: {c}\r\n"))
        .unwrap_or_default();
    raw(
        addr,
        &format!("{method} {path} HTTP/1.1\r\nHost: {addr}\r\n{ct}"),
        body,
    )
}

fn json_call(addr: SocketAddr, method: &str, path: &str, body: &Value) -> (u16, Value) {
    let ct = (method != "GET").then_some("application/json");
    let b = if method == "GET" {
        Vec::new()
    } else {
        body.to_string().into_bytes()
    };
    let (st, out) = call(addr, method, path, ct, &b);
    (
        st,
        serde_json::from_slice(&out)
            .unwrap_or_else(|_| panic!("not JSON: {}", String::from_utf8_lossy(&out))),
    )
}

fn stop(r: Running) -> Summary {
    let (st, _) = json_call(r.addr, "POST", "/v1/shutdown", &json!({}));
    assert_eq!(st, 200);
    r.join.join().unwrap()
}

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

fn request(seed: &str) -> Value {
    json!({
        "kind": "harness", "workload": "w.toml", "backend": "mockllm", "model": "mock-auto",
        "mode": "sim", "arm": "treatment", "replicates": 1, "seed": seed
    })
}

/// Cites: CTL-24
#[test]
fn the_committed_openapi_document_is_the_route_tables() {
    let path = Path::new(env!("CARGO_MANIFEST_DIR")).join("openapi.json");
    if std::env::var("ACN_WRITE_OPENAPI").is_ok() {
        std::fs::write(&path, acn_ctl::api::openapi_text()).unwrap();
    }
    assert_eq!(
        std::fs::read_to_string(&path).unwrap(),
        acn_ctl::api::openapi_text()
    );
}

/// Cites: CTL-2, CTL-3, CTL-24
#[test]
fn every_route_answers_json_and_origins_types_and_ids_are_checked() {
    let r = serve();
    let a = r.addr;
    // The router serves every route of the table (none answers the fallback).
    for route in acn_ctl::api::ROUTES {
        let path = route
            .path
            .replace("{hash}", &"0".repeat(64))
            .replace("{request_id}", &"0".repeat(64))
            .replace("{run_id}", &"0".repeat(64))
            .replace("{*path}", "manifest.json")
            .replace("{name}", "x");
        // Shutdown stops the server; the document is the OpenAPI document
        // itself, the one other answer that is not `{ok, ...}` (ADR-39).
        if route.op == "shutdown" || route.op == "openapi" {
            continue;
        }
        let m = route.method.as_str().to_uppercase();
        let (st, out) = call(a, &m, &path, route.body, b"{}");
        assert_ne!(st, 405, "{m} {path}: the router does not take its method");
        // Every answer but a bundle file's bytes is one JSON object.
        let v: Value = if route.op == "get_bundle_file" {
            serde_json::from_slice(&out).unwrap_or(json!({"ok": "bytes"}))
        } else {
            serde_json::from_slice(&out)
                .unwrap_or_else(|_| panic!("{m} {path}: {}", String::from_utf8_lossy(&out)))
        };
        assert!(v.get("ok").is_some(), "{m} {path}: {v}");
        assert_ne!(v["code"], "not_found", "{m} {path} hit the fallback ({st})");
    }
    // axum's own refusals are JSON too: a method a path does not take, and a
    // body over the limit (CTL-2).
    let (st, v) = json_call(a, "PATCH", "/v1/runs", &json!({}));
    assert_eq!((st, v["code"].as_str()), (405, Some("method_not_allowed")));
    let big = vec![b' '; (1 << 20) + 1];
    let (st, out) = call(a, "POST", "/v1/runs", Some("application/json"), &big);
    let v: Value = serde_json::from_slice(&out).unwrap();
    assert_eq!((st, v["code"].as_str()), (413, Some("payload_too_large")));
    // A route that takes no body takes only `{}` or nothing.
    let (st, v) = json_call(a, "DELETE", "/v1/endpoints/none", &json!({"force": true}));
    assert_eq!((st, v["code"].as_str()), (400, Some("bad_request")));
    let (st, _) = call(
        a,
        "DELETE",
        "/v1/endpoints/none",
        Some("application/json"),
        b"",
    );
    assert_eq!(st, 404);
    let (st, doc) = json_call(a, "GET", "/v1/openapi.json", &json!({}));
    assert_eq!((st, doc["openapi"].as_str()), (200, Some("3.1.0")));
    // An unknown path: one JSON object, 404.
    let (st, v) = json_call(a, "GET", "/v1/nothing", &json!({}));
    assert_eq!((st, v["code"].as_str()), (404, Some("not_found")));
    // A wrong or missing Host (CTL-3).
    let (st, out) = raw(a, "GET /v1/runs HTTP/1.1\r\nHost: evil.example:80\r\n", b"");
    assert_eq!(st, 421, "{}", String::from_utf8_lossy(&out));
    let (st, _) = raw(a, "GET /v1/runs HTTP/1.1\r\n", b"");
    assert!(st == 421 || st == 400, "{st}");
    // Two Hosts, one of them right, are not this server's (CTL-3).
    let (st, _) = raw(
        a,
        &format!("GET /v1/runs HTTP/1.1\r\nHost: {a}\r\nHost: evil.example:80\r\n"),
        b"",
    );
    assert!(st == 421 || st == 400, "{st}");
    // A plain form cannot start a run or stop the server.
    let (st, _) = call(
        a,
        "POST",
        "/v1/runs",
        Some("application/x-www-form-urlencoded"),
        b"a=b",
    );
    assert_eq!(st, 415);
    let (st, _) = call(a, "POST", "/v1/shutdown", Some("text/plain"), b"");
    assert_eq!(st, 415);
    // Ids are checked before they touch a path.
    let (st, v) = json_call(a, "GET", "/v1/runs/..%2F..%2Fetc", &json!({}));
    assert_eq!((st, v["code"].as_str()), (400, Some("bad_id")));
    let (st, v) = json_call(a, "GET", "/v1/bundles/ABC", &json!({}));
    assert_eq!((st, v["code"].as_str()), (400, Some("bad_id")));
    // An unknown field (CTL-2).
    let mut bad = request("7");
    bad["keep_content"] = true.into();
    let (st, v) = json_call(a, "POST", "/v1/runs", &bad);
    assert_eq!((st, v["code"].as_str()), (400, Some("bad_request")));
    let s = stop(r);
    assert!(s.ok);
    assert!(s.requests > 10);
}

/// Cites: CTL-21, CTL-22, CTL-23, CTL-1
#[test]
fn a_run_through_the_api_is_served_as_its_manifest_lists_it() {
    let r = serve();
    let a = r.addr;
    let (st, v) = json_call(a, "POST", "/v1/runs", &request("7"));
    assert_eq!(st, 202, "{v}");
    let id = v["request_id"].as_str().unwrap().to_owned();
    let path = format!("/v1/runs/{id}");
    assert!(wait(|| json_call(a, "GET", &path, &json!({})).1["status"]
        ["state"]
        == "done"));
    let (_, v) = json_call(a, "GET", &path, &json!({}));
    let run_id = v["status"]["run_id"].as_str().unwrap().to_owned();
    // A resubmission: 200, the same request.
    let (st, again) = json_call(a, "POST", "/v1/runs", &request("7"));
    assert_eq!((st, again["request_id"].as_str()), (200, Some(id.as_str())));
    let (_, list) = json_call(a, "GET", "/v1/runs", &json!({}));
    assert_eq!(list["runs"].as_array().unwrap().len(), 1);
    // The bundle, verified, with its files.
    let (st, b) = json_call(a, "GET", &format!("/v1/bundles/{run_id}"), &json!({}));
    assert_eq!(st, 200, "{b}");
    assert_eq!(b["run_id"], run_id.as_str());
    let files = b["files"].as_array().unwrap();
    assert!(!files.is_empty());
    let f = files.iter().find(|f| f["path"] == "spans.parquet").unwrap();
    let (st, bytes) = call(
        a,
        "GET",
        &format!("/v1/bundles/{run_id}/files/spans.parquet"),
        None,
        b"",
    );
    assert_eq!(st, 200);
    assert_eq!(
        blake3::hash(&bytes).to_hex().as_str(),
        f["hash"].as_str().unwrap()
    );
    // Only what the manifest lists.
    let (st, v) = json_call(
        a,
        "GET",
        &format!("/v1/bundles/{run_id}/files/logs/x"),
        &json!({}),
    );
    assert_eq!((st, v["code"].as_str()), (404, Some("unknown_file")));
    let (st, _) = json_call(
        a,
        "GET",
        &format!("/v1/bundles/{run_id}/files/..%2F..%2Fw.toml"),
        &json!({}),
    );
    assert_eq!(st, 404);
    // A file that no longer has its hash is not served, and the bundle is invalid.
    let bundle = r.dir.path().join("runs").join(&run_id);
    std::fs::write(bundle.join("spans.parquet"), b"changed").unwrap();
    let (st, v) = json_call(
        a,
        "GET",
        &format!("/v1/bundles/{run_id}/files/spans.parquet"),
        &json!({}),
    );
    assert_eq!((st, v["code"].as_str()), (409, Some("bundle_invalid")));
    let (st, v) = json_call(a, "GET", &format!("/v1/bundles/{run_id}"), &json!({}));
    assert_eq!((st, v["code"].as_str()), (409, Some("bundle_invalid")));
    let s = stop(r);
    assert!(s.ok);
    assert_eq!(s.run_ids, [run_id]);
}

/// Cites: CTL-20
#[test]
fn a_scenario_is_stored_by_its_hash_and_a_trace_driven_one_is_refused() {
    let r = serve();
    let a = r.addr;
    let toml = "schema_version = 1\nname = \"p\"\n\n[[link]]\nname = \"p\"\ndirection = \"up\"\n\n[[link]]\nname = \"p\"\ndirection = \"down\"\n";
    let (st, out) = call(
        a,
        "POST",
        "/v1/scenarios",
        Some("application/toml"),
        toml.as_bytes(),
    );
    let v: Value = serde_json::from_slice(&out).unwrap();
    assert_eq!(st, 200, "{v}");
    let hash = blake3::hash(toml.as_bytes()).to_hex().to_string();
    assert_eq!(v["hash"], hash.as_str());
    let (st, g) = json_call(a, "GET", &format!("/v1/scenarios/{hash}"), &json!({}));
    assert_eq!(
        (st, g["toml"].as_str(), g["name"].as_str()),
        (200, Some(toml), Some("p"))
    );
    // Again: a no-op.
    let (st, _) = call(
        a,
        "POST",
        "/v1/scenarios",
        Some("application/toml"),
        toml.as_bytes(),
    );
    assert_eq!(st, 200);
    // A run on it, named by its hash.
    let mut q = request("7");
    q["scenario"] = json!({"hash": hash});
    let (st, v) = json_call(a, "POST", "/v1/runs", &q);
    assert_eq!(st, 202, "{v}");
    let traced = toml.replace(
        "direction = \"down\"\n",
        "direction = \"down\"\n\n[link.trace]\ndir = \"x\"\n",
    );
    let (st, out) = call(
        a,
        "POST",
        "/v1/scenarios",
        Some("application/toml"),
        traced.as_bytes(),
    );
    let v: Value = serde_json::from_slice(&out).unwrap();
    assert_eq!((st, v["code"].as_str()), (400, Some("trace_driven")));
    let (st, out) = call(
        a,
        "POST",
        "/v1/scenarios",
        Some("application/toml"),
        b"name = 3",
    );
    let v: Value = serde_json::from_slice(&out).unwrap();
    assert_eq!((st, v["code"].as_str()), (400, Some("scenario_invalid")));
    let (st, _) = call(
        a,
        "POST",
        "/v1/scenarios",
        Some("application/json"),
        toml.as_bytes(),
    );
    assert_eq!(st, 415);
    let (st, v) = json_call(
        a,
        "GET",
        &format!("/v1/scenarios/{}", "1".repeat(64)),
        &json!({}),
    );
    assert_eq!((st, v["code"].as_str()), (404, Some("unknown_scenario")));
    stop(r);
}

/// Cites: CTL-30
#[test]
fn endpoints_are_named_checked_and_held_while_a_request_uses_them() {
    // The run waits at the gate until the test has tried the delete.
    let (open_tx, open_rx) = std::sync::mpsc::channel::<()>();
    let (at_tx, at_rx) = std::sync::mpsc::channel::<()>();
    let open_rx = std::sync::Mutex::new(open_rx);
    let r = serve_with(move |c| {
        c.with_before_run(std::sync::Arc::new(move || {
            let _ = at_tx.send(());
            let _ = open_rx.lock().unwrap().recv();
        }))
    });
    let a = r.addr;
    let (st, v) = json_call(
        a,
        "PUT",
        "/v1/endpoints/mock",
        &json!({"backend": "mockllm", "url": "acn-mock://loopback"}),
    );
    assert_eq!(st, 200, "{v}");
    let (st, v) = json_call(
        a,
        "PUT",
        "/v1/endpoints/Bad_Name",
        &json!({"backend": "mockllm", "url": "acn-mock://loopback"}),
    );
    assert_eq!((st, v["code"].as_str()), (400, Some("bad_id")));
    // Credentials in a URL are refused, and the error does not echo them.
    let (st, v) = json_call(
        a,
        "PUT",
        "/v1/endpoints/creds",
        &json!({"backend": "openai", "url": "https://user:hunter2@api.example.com"}),
    );
    assert_eq!(st, 400);
    assert!(!v.to_string().contains("hunter2"), "{v}");
    let (st, _) = json_call(
        a,
        "PUT",
        "/v1/endpoints/x",
        &json!({"backend": "openai", "url": "acn-mock://loopback"}),
    );
    assert_eq!(st, 400);
    let (_, l) = json_call(a, "GET", "/v1/endpoints", &json!({}));
    assert_eq!(l["endpoints"].as_array().unwrap().len(), 1);
    // A running request names it: it cannot be deleted meanwhile.
    let mut q = request("7");
    q["mode"] = "live".into();
    q["opt"] = json!({"endpoint_name": "mock"});
    let (st, v) = json_call(a, "POST", "/v1/runs", &q);
    assert_eq!(st, 202, "{v}");
    let id = v["request_id"].as_str().unwrap().to_owned();
    at_rx
        .recv_timeout(std::time::Duration::from_secs(60))
        .unwrap();
    let (st, v) = json_call(a, "DELETE", "/v1/endpoints/mock", &json!({}));
    assert_eq!((st, v["code"].as_str()), (409, Some("endpoint_in_use")));
    let (st, _) = json_call(a, "DELETE", "/v1/endpoints/none", &json!({}));
    assert_eq!(st, 404);
    open_tx.send(()).unwrap();
    let path = format!("/v1/runs/{id}");
    assert!(wait(|| json_call(a, "GET", &path, &json!({})).1["status"]
        ["state"]
        == "done"));
    // Done, it no longer holds the endpoint.
    let (st, v) = json_call(a, "DELETE", "/v1/endpoints/mock", &json!({}));
    assert_eq!(st, 200, "{v}");
    stop(r);
}

/// Cites: CTL-2, HAR-22
#[test]
fn no_answer_carries_a_credential_the_environment_holds() {
    // `set_var` is unsafe: the check runs in a child with the key set.
    if std::env::var("ACN_CTL_CHILD").is_ok() {
        // The child logs as the binary does, so its stderr is checked too.
        tracing_subscriber::fmt()
            .with_writer(std::io::stderr)
            .with_max_level(tracing::Level::DEBUG)
            .init();
        let secret = std::env::var("OPENAI_API_KEY").unwrap();
        let r = serve();
        let a = r.addr;
        let mut seen = String::new();
        let (_, v) = json_call(
            a,
            "PUT",
            "/v1/endpoints/oai",
            &json!({"backend": "openai", "url": "https://api.example.invalid"}),
        );
        seen.push_str(&v.to_string());
        let mut q = request("7");
        q["backend"] = "openai".into();
        q["mode"] = "live".into();
        q["opt"] = json!({"endpoint_name": "oai", "request_timeout_ms": 1000, "max_retries": 0});
        let (st, v) = json_call(a, "POST", "/v1/runs", &q);
        seen.push_str(&v.to_string());
        assert_eq!(st, 202, "{v}");
        let id = v["request_id"].as_str().unwrap().to_owned();
        let path = format!("/v1/runs/{id}");
        // Without `real-api` the run is refused; with it, the host does not
        // resolve. Either way it fails, and its error is served.
        assert!(wait(|| json_call(a, "GET", &path, &json!({})).1["status"]
            ["state"]
            == "failed"));
        seen.push_str(&json_call(a, "GET", &path, &json!({})).1.to_string());
        for p in ["/v1/runs", "/v1/endpoints", "/v1/openapi.json"] {
            seen.push_str(&json_call(a, "GET", p, &json!({})).1.to_string());
        }
        stop(r);
        assert!(!seen.contains(&secret), "a response carries the credential");
        return;
    }
    let out = std::process::Command::new(std::env::current_exe().unwrap())
        .args([
            "--exact",
            "no_answer_carries_a_credential_the_environment_holds",
            "--nocapture",
        ])
        .env("ACN_CTL_CHILD", "1")
        .env("OPENAI_API_KEY", "sk-ctl-test-secret-0000")
        .output()
        .unwrap();
    assert!(
        out.status.success(),
        "{}",
        String::from_utf8_lossy(&out.stdout)
    );
    assert!(!String::from_utf8_lossy(&out.stderr).contains("sk-ctl-test-secret-0000"));
}

/// Cites: CTL-1, CTL-12
#[test]
fn one_server_holds_a_runs_directory_and_one_that_cannot_bind_touches_nothing() {
    let r = serve();
    // A second registry on the same runs directory is refused.
    let err = Ctl::open(config(r.dir.path())).err().unwrap();
    assert_eq!((err.status, err.code), (409, "registry_in_use"));
    // A server that cannot bind opens no registry.
    let other = tempfile::tempdir().unwrap();
    let err = Server::bind(config(other.path()), r.addr.port())
        .err()
        .unwrap();
    assert_eq!(err.code, "bind");
    assert!(!other.path().join("runs").exists());
    stop(r);
}

/// Cites: CTL-1, CTL-21
#[test]
fn a_stopping_server_serves_reads_and_refuses_writes() {
    let mut held = None;
    let r = serve_with(|c| {
        held = Some(c.clone());
        c
    });
    let a = r.addr;
    // The registry stops as a signal stops it, before the listener closes.
    held.unwrap().stop();
    let (st, v) = json_call(
        a,
        "PUT",
        "/v1/endpoints/mock",
        &json!({"backend": "mockllm", "url": "acn-mock://loopback"}),
    );
    assert_eq!((st, v["code"].as_str()), (503, Some("shutting_down")));
    let (st, v) = json_call(a, "POST", "/v1/runs", &request("7"));
    assert_eq!((st, v["code"].as_str()), (503, Some("shutting_down")));
    let (st, _) = json_call(a, "GET", "/v1/runs", &json!({}));
    assert_eq!(st, 200);
    let s = stop(r);
    assert!(s.ok);
}
