//! HAR-50: `acn harness run` executes one cell and one arm into one bundle and
//! prints one JSON object (CON-8).

#![allow(clippy::unwrap_used, clippy::expect_used, clippy::panic)] // CON-19: tests are exempt

use std::process::Command;

const SMOKE: &str = concat!(
    env!("CARGO_MANIFEST_DIR"),
    "/../../workloads/harness-smoke.toml"
);

fn acn(args: &[&str]) -> (Option<i32>, serde_json::Value) {
    let out = Command::new(env!("CARGO_BIN_EXE_acn"))
        .args(args)
        .output()
        .expect("spawn acn");
    let stdout = String::from_utf8(out.stdout).expect("utf8");
    (
        out.status.code(),
        serde_json::from_str(stdout.trim()).unwrap_or_else(|e| panic!("{e}: {stdout}")),
    )
}

/// Cites: HAR-50, CON-8
#[test]
fn harness_run_writes_one_bundle_and_prints_its_identity() {
    let runs = tempfile::tempdir().unwrap();
    let runs_dir = runs.path().to_str().unwrap();
    let args = [
        "harness",
        "run",
        "--workload",
        SMOKE,
        "--backend",
        "mockllm",
        "--model",
        "mock-auto",
        "--seed",
        "3",
        "--replicates",
        "2",
        "--vary",
        "tool_order_stable=false",
        "--runs-dir",
        runs_dir,
    ];
    let (code, json) = acn(&args);
    assert_eq!(code, Some(0), "{json}");
    assert_eq!(json["ok"], true);
    let run_id = json["run_id"].as_str().unwrap();
    assert_eq!(run_id.len(), 64);
    assert_eq!(json["bundle_digest"].as_str().unwrap().len(), 64);
    let dir = runs.path().join(run_id);
    let (_, verified) = acn(&["bundle", "verify", "--views", dir.to_str().unwrap()]);
    assert_eq!(verified["run_id"], run_id);
    assert_eq!(verified["bundle_digest"], json["bundle_digest"]);
    // The same command again: the bundle is never replaced (CON-29).
    let (code, again) = acn(&args);
    assert_eq!(code, Some(1));
    assert!(again["error"].as_str().unwrap().contains("never replaced"));
    // A misspelt knob is refused before anything runs.
    let (code, bad) = acn(&[
        "harness",
        "run",
        "--workload",
        SMOKE,
        "--backend",
        "mockllm",
        "--model",
        "mock-auto",
        "--seed",
        "3",
        "--vary",
        "tool_order=false",
        "--runs-dir",
        runs_dir,
    ]);
    assert_eq!(code, Some(1));
    assert!(bad["error"].as_str().unwrap().contains("not a knob"));
}

/// The mock over HTTP on a virtual clock, from a thread of its own.
fn mock_server() -> String {
    let (tx, rx) = std::sync::mpsc::channel();
    std::thread::spawn(move || {
        let rt = tokio::runtime::Builder::new_current_thread()
            .enable_all()
            .build()
            .unwrap();
        rt.block_on(async move {
            let listener = tokio::net::TcpListener::bind("127.0.0.1:0").await.unwrap();
            tx.send(listener.local_addr().unwrap()).unwrap();
            acn_mockllm::server::serve(
                listener,
                acn_mockllm::Mock::new(1).unwrap(),
                std::sync::Arc::new(acn_emu::clock::SimClock::new()),
            )
            .await
            .unwrap();
        });
    });
    format!("http://{}", rx.recv().unwrap())
}

/// Cites: HAR-25, HAR-22, CON-29
#[test]
fn proxy_variables_change_nothing_about_a_live_run() {
    let runs = tempfile::tempdir().unwrap();
    let workload = runs.path().join("w.toml");
    let fast = std::fs::read_to_string(SMOKE)
        .unwrap()
        .replace(
            "think_time_ns = { min = 1_000_000_000, max = 3_000_000_000 }",
            "think_time_ns = { min = 0, max = 0 }",
        )
        .replace(
            "think_time_ns = { min = 500_000_000, max = 1_500_000_000 }",
            "think_time_ns = { min = 0, max = 0 }",
        );
    std::fs::write(&workload, fast).unwrap();
    let endpoint = mock_server();
    // A proxy that would refuse every connection: a run that honoured it fails.
    let out = Command::new(env!("CARGO_BIN_EXE_acn"))
        .args([
            "harness",
            "run",
            "--workload",
            workload.to_str().unwrap(),
            "--backend",
            "mockllm",
            "--model",
            "mock-auto",
            "--mode",
            "live",
            "--seed",
            "5",
            "--endpoint",
            &endpoint,
            "--runs-dir",
            runs.path().to_str().unwrap(),
        ])
        .env("HTTP_PROXY", "http://127.0.0.1:9")
        .env("HTTPS_PROXY", "http://127.0.0.1:9")
        .env("ALL_PROXY", "http://127.0.0.1:9")
        .env("http_proxy", "http://127.0.0.1:9")
        .env("all_proxy", "http://127.0.0.1:9")
        .output()
        .unwrap();
    let json: serde_json::Value =
        serde_json::from_str(String::from_utf8(out.stdout).unwrap().trim()).unwrap();
    assert_eq!(json["ok"], true, "{json}");
}
