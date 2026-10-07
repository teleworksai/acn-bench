//! SPEC 070 acceptance (CTL-1, CON-8, CON-29): runs started through
//! `acn ctl serve` are the runs the CLI starts, and the server stops as
//! CTL-1 says.
//!
//! It drives the `acn` binary, so it is a test of `acn-cli`'s package
//! (`cargo test -p acn-cli --test accept_ctl`; ADR-39).

#![allow(clippy::unwrap_used, clippy::expect_used, clippy::panic)] // CON-19: tests are exempt

use std::cell::Cell;
use std::io::{BufRead as _, Read as _, Write as _};
use std::path::Path;
use std::process::{Child, Command, Stdio};
use std::sync::{Arc, Mutex};

use serde_json::{Value, json};

const SHEET: &str = r#"schema_version = 1
placeholder = true
doc = "the control plane's acceptance sheet"
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

/// A two-link scenario (SPEC 020 §3) every call crosses.
const SCENARIO: &str = "schema_version = 1\nname = \"p\"\n\n[[link]]\nname = \"p\"\ndirection = \"up\"\n\n[[link]]\nname = \"p\"\ndirection = \"down\"\n";

/// A scratch directory inside this checkout, the workspace root (CON-28):
/// the binary refuses a root whose frozen code is not the code it was built
/// from (CON-31). It holds a workload and a sheet, and is removed on drop.
struct Scratch {
    dir: tempfile::TempDir,
    /// The directory, relative to the root, as the server resolves paths.
    rel: String,
}

fn workspace() -> Scratch {
    let root = std::fs::canonicalize(concat!(env!("CARGO_MANIFEST_DIR"), "/../..")).unwrap();
    let base = root.join("target/accept-ctl");
    std::fs::create_dir_all(&base).unwrap();
    let dir = tempfile::tempdir_in(&base).unwrap();
    let rel = dir
        .path()
        .strip_prefix(&root)
        .unwrap()
        .to_string_lossy()
        .into_owned();
    std::fs::copy(
        root.join("workloads/harness-smoke.toml"),
        dir.path().join("w.toml"),
    )
    .unwrap();
    std::fs::write(dir.path().join("sheet.toml"), SHEET).unwrap();
    std::fs::write(dir.path().join("p.toml"), SCENARIO).unwrap();
    Scratch { dir, rel }
}

/// One `acn` command: its exit code and its one JSON object (CON-8).
fn acn(dir: &Path, args: &[&str]) -> (Option<i32>, Value) {
    let out = Command::new(env!("CARGO_BIN_EXE_acn"))
        .current_dir(dir)
        .args(args)
        .output()
        .unwrap();
    let stdout = String::from_utf8(out.stdout).unwrap();
    let v = serde_json::from_str(stdout.trim())
        .unwrap_or_else(|e| panic!("stdout is not one JSON object: {e}\n{stdout}"));
    (out.status.code(), v)
}

fn pause(ms: u64) {
    let (_tx, rx) = std::sync::mpsc::channel::<()>();
    let _ = rx.recv_timeout(std::time::Duration::from_millis(ms));
}

fn wait(cond: impl Fn() -> bool) -> bool {
    for _ in 0..3000 {
        if cond() {
            return true;
        }
        pause(20);
    }
    cond()
}

/// A running `acn ctl serve`: killed and reaped if a test fails before it
/// stops, so no server outlives its test.
struct Served {
    child: Option<Child>,
    addr: String,
    /// The API calls this test made, a lower bound of the summary's count.
    calls: Cell<u64>,
    log: Arc<Mutex<Vec<String>>>,
    reader: Option<std::thread::JoinHandle<()>>,
}

impl Drop for Served {
    fn drop(&mut self) {
        if let Some(mut c) = self.child.take() {
            let _ = c.kill();
            let _ = c.wait();
        }
    }
}

fn serve(w: &Scratch) -> Served {
    let runs = format!("{}/runs", w.rel);
    let mut child = Command::new(env!("CARGO_BIN_EXE_acn"))
        .current_dir(w.dir.path())
        .args(["ctl", "serve", "--port", "0", "--runs-dir", &runs])
        .stdout(Stdio::piped())
        .stderr(Stdio::piped())
        .spawn()
        .unwrap();
    // CTL-1: the address and every request are logged on stderr. The log is
    // kept, in order, by a thread, so the server never blocks on the pipe.
    let stderr = child.stderr.take().unwrap();
    let log = Arc::new(Mutex::new(Vec::new()));
    let (tx, rx) = std::sync::mpsc::channel();
    let reader = std::thread::spawn({
        let log = log.clone();
        move || {
            for line in std::io::BufReader::new(stderr).lines() {
                let Ok(line) = line else { break };
                if line.contains("acn ctl listening") {
                    let _ = tx.send(line.clone());
                }
                log.lock().unwrap().push(line);
            }
        }
    });
    let mut served = Served {
        child: Some(child),
        addr: String::new(),
        calls: Cell::new(0),
        log,
        reader: Some(reader),
    };
    let line = rx
        .recv_timeout(std::time::Duration::from_secs(60))
        .expect("the server logs its address");
    let at = line.find("addr=").map_or(0, |i| i + 5);
    served.addr = line[at..]
        .chars()
        .take_while(|c| !c.is_whitespace())
        .collect();
    served
}

impl Served {
    /// One HTTP/1.1 request, raw: its status and JSON body.
    fn call(&self, method: &str, path: &str, body: Option<&Value>) -> (u16, Value) {
        self.calls.set(self.calls.get() + 1);
        let mut s = std::net::TcpStream::connect(&self.addr).unwrap();
        s.set_read_timeout(Some(std::time::Duration::from_secs(60)))
            .unwrap();
        let b = body.map(Value::to_string).unwrap_or_default();
        let ct = if body.is_some() {
            "Content-Type: application/json\r\n"
        } else {
            ""
        };
        write!(
            s,
            "{method} {path} HTTP/1.1\r\nHost: {}\r\n{ct}Content-Length: {}\r\nConnection: close\r\n\r\n{b}",
            self.addr,
            b.len()
        )
        .unwrap();
        let mut out = Vec::new();
        s.read_to_end(&mut out).unwrap();
        let split = out.windows(4).position(|w| w == b"\r\n\r\n").unwrap();
        let head = String::from_utf8_lossy(&out[..split]).into_owned();
        let status = head.split(' ').nth(1).unwrap().parse().unwrap();
        (status, serde_json::from_slice(&out[split + 4..]).unwrap())
    }

    fn status(&self, request_id: &str) -> Value {
        self.call("GET", &format!("/v1/runs/{request_id}"), None).1["status"].clone()
    }

    /// Submit `req`; its request_id.
    fn submit(&self, req: &Value) -> String {
        let (st, v) = self.call("POST", "/v1/runs", Some(req));
        assert_eq!(st, 202, "{v}");
        v["request_id"].as_str().unwrap().to_owned()
    }

    /// Submit `req` and wait for it to finish; its status.
    fn run(&self, req: &Value) -> Value {
        let id = self.submit(req);
        assert!(wait(|| matches!(
            self.status(&id)["state"].as_str(),
            Some("done" | "failed")
        )));
        let s = self.status(&id);
        assert_eq!(s["state"], "done", "{s}");
        s
    }

    /// Wait for the server to exit, at most two minutes: its exit code, the
    /// one object it printed (CTL-1, CON-8), and its log.
    fn finish(mut self) -> (Option<i32>, Value, Vec<String>) {
        let mut child = self.child.take().unwrap();
        let mut status = None;
        for _ in 0..1200 {
            status = child.try_wait().unwrap();
            if status.is_some() {
                break;
            }
            pause(100);
        }
        let Some(status) = status else {
            let _ = child.kill();
            let _ = child.wait();
            panic!("the server did not stop");
        };
        let mut stdout = String::new();
        child
            .stdout
            .take()
            .unwrap()
            .read_to_string(&mut stdout)
            .unwrap();
        if let Some(r) = self.reader.take() {
            r.join().unwrap();
        }
        let v = serde_json::from_str(stdout.trim())
            .unwrap_or_else(|e| panic!("stdout is not one JSON object: {e}\n{stdout}"));
        let log = self.log.lock().unwrap().clone();
        (status.code(), v, log)
    }
}

/// The summary is the server's: its address, at least the calls made, ok.
fn summary_is(sum: &Value, addr: &str, calls: u64) {
    assert_eq!(sum["ok"], true, "{sum}");
    assert_eq!(sum["addr"], addr, "{sum}");
    assert!(sum["requests"].as_u64().unwrap() >= calls, "{sum}");
}

/// A CLI run's identity is the server's (CON-29): both are set, the same,
/// and the server's run is its own, not adopted.
fn same_run(cli: &Value, srv: &Value) {
    for k in ["run_id", "bundle_digest"] {
        assert!(cli[k].is_string() && srv[k].is_string(), "{k}: {cli} {srv}");
        assert_eq!(cli[k], srv[k], "{k}: {cli} {srv}");
    }
    assert_eq!(srv["reused"], false, "{srv}");
}

/// Cites: CTL-1, CTL-10, CON-8, CON-29
#[test]
fn runs_through_the_server_are_the_runs_the_cli_starts() {
    let w = workspace();
    let d = w.dir.path();
    let at = |f: &str| format!("{}/{f}", w.rel);
    // From the CLI, into its own runs directory: a default harness run, a
    // generator run, and a harness run with every input set otherwise.
    let harness = [
        "harness",
        "run",
        "--workload",
        "w.toml",
        "--backend",
        "mockllm",
    ];
    let (code, h) = acn(
        d,
        &[
            &harness[..],
            &[
                "--model",
                "mock-auto",
                "--seed",
                "7",
                "--runs-dir",
                "cli-runs",
            ],
        ]
        .concat(),
    );
    assert_eq!(code, Some(0), "{h}");
    let (code, g) = acn(
        d,
        &[
            "gen",
            "run",
            "--sheet",
            "sheet.toml",
            "--seed",
            "3",
            "--runs-dir",
            "cli-runs",
        ],
    );
    assert_eq!(code, Some(0), "{g}");
    let (code, x) = acn(
        d,
        &[
            &harness[..],
            &[
                "--model",
                "mock-auto",
                "--seed",
                "11",
                "--arm",
                "control",
                "--replicates",
                "2",
                "--vary",
                "fanout_prompting=per_child",
                "--max-retries",
                "1",
                "--stall-threshold-ms",
                "100.5",
                "--scenario",
                "p.toml",
                "--runs-dir",
                "cli-runs",
            ],
        ]
        .concat(),
    );
    assert_eq!(code, Some(0), "{x}");
    // The same runs through the server, the scenario by its stored hash.
    let s = serve(&w);
    let base = |kind: &str, seed: &str| json!({"kind": kind, "mode": "sim", "arm": "treatment", "replicates": 1, "seed": seed});
    let mut hq = base("harness", "7");
    hq["workload"] = at("w.toml").into();
    hq["backend"] = "mockllm".into();
    hq["model"] = "mock-auto".into();
    let hs = s.run(&hq);
    let mut gq = base("generator", "3");
    gq["sheet"] = at("sheet.toml").into();
    let gs = s.run(&gq);
    let mut stored = std::net::TcpStream::connect(&s.addr).unwrap();
    write!(
        stored,
        "POST /v1/scenarios HTTP/1.1\r\nHost: {}\r\nContent-Type: application/toml\r\nContent-Length: {}\r\nConnection: close\r\n\r\n{SCENARIO}",
        s.addr,
        SCENARIO.len()
    )
    .unwrap();
    let mut out = String::new();
    stored.read_to_string(&mut out).unwrap();
    assert!(out.starts_with("HTTP/1.1 200"), "{out}");
    s.calls.set(s.calls.get() + 1);
    let hash = blake3::hash(SCENARIO.as_bytes()).to_hex().to_string();
    let mut xq = hq.clone();
    xq["seed"] = "11".into();
    xq["arm"] = "control".into();
    xq["replicates"] = 2.into();
    xq["vary"] = json!({"fanout_prompting": "per_child"});
    xq["opt"] = json!({"max_retries": 1, "stall_threshold_ms": 100.5});
    xq["scenario"] = json!({"hash": hash});
    let xs = s.run(&xq);
    same_run(&h, &hs);
    same_run(&g, &gs);
    same_run(&x, &xs);
    let (st, _) = s.call("POST", "/v1/shutdown", Some(&json!({})));
    assert_eq!(st, 200);
    let (addr, calls) = (s.addr.clone(), s.calls.get());
    let (code, sum, _) = s.finish();
    assert_eq!(code, Some(0), "{sum}");
    summary_is(&sum, &addr, calls);
    let mut want = vec![
        hs["run_id"].clone(),
        gs["run_id"].clone(),
        xs["run_id"].clone(),
    ];
    want.sort_by(|a, b| a.as_str().cmp(&b.as_str()));
    assert_eq!(sum["run_ids"], Value::Array(want));
}

enum Stop {
    #[cfg(unix)]
    Sigterm,
    Shutdown,
}

/// Stop the server while a live run is in progress: it finishes the run,
/// exits 0 and prints the run's id; the run ended after the server began
/// stopping, and its bundle verifies (CTL-1).
fn stops_after_the_run_in_progress(how: &Stop) {
    let w = workspace();
    let s = serve(&w);
    assert!(s.addr.starts_with("127.0.0.1:"), "{}", s.addr);
    // A specific loopback bind: another loopback address is not served.
    // (macOS routes only 127.0.0.1 by default, so the probe is Linux's.)
    #[cfg(target_os = "linux")]
    {
        let port = s.addr.rsplit(':').next().unwrap();
        assert!(std::net::TcpStream::connect(format!("127.0.0.2:{port}")).is_err());
    }
    // A live run on the served mock waits on the wall clock (HAR-26).
    let id = s.submit(&json!({
        "kind": "harness", "workload": format!("{}/w.toml", w.rel), "backend": "mockllm",
        "model": "mock-auto", "mode": "live", "arm": "treatment", "replicates": 1, "seed": "7",
        "opt": {"endpoint": "acn-mock://loopback"}
    }));
    assert!(wait(|| s.status(&id)["state"] != "queued"));
    let now = s.status(&id);
    assert_eq!(now["state"], "running", "the run is in progress: {now}");
    let run_id = now["run_id"].clone();
    assert!(run_id.is_string(), "{now}");
    match how {
        #[cfg(unix)]
        Stop::Sigterm => {
            let pid = s.child.as_ref().unwrap().id().to_string();
            assert!(
                Command::new("kill")
                    .args(["-TERM", &pid])
                    .status()
                    .unwrap()
                    .success()
            );
        }
        Stop::Shutdown => {
            let (st, _) = s.call("POST", "/v1/shutdown", Some(&json!({})));
            assert_eq!(st, 200);
        }
    }
    let (addr, calls) = (s.addr.clone(), s.calls.get());
    let (code, sum, log) = s.finish();
    assert_eq!(code, Some(0), "{sum}");
    summary_is(&sum, &addr, calls);
    assert_eq!(sum["run_ids"], json!([run_id]));
    // The run finished after the server began to stop: it was in progress.
    let stopping = log.iter().position(|l| l.contains("acn ctl stopping"));
    let done = log
        .iter()
        .position(|l| l.contains("acn ctl run done") && l.contains(&id));
    assert!(
        matches!((stopping, done), (Some(a), Some(b)) if a < b),
        "{stopping:?} {done:?}\n{}",
        log.join("\n")
    );
    let bundle = format!("runs/{}", run_id.as_str().unwrap());
    let (code, v) = acn(w.dir.path(), &["bundle", "verify", &bundle]);
    assert_eq!(code, Some(0), "{v}");
}

/// Cites: CTL-1, CON-8
#[cfg(unix)]
#[test]
fn on_sigterm_the_server_finishes_the_run_in_progress_and_prints_it() {
    stops_after_the_run_in_progress(&Stop::Sigterm);
}

/// Cites: CTL-1, CON-8
#[test]
fn on_shutdown_the_server_finishes_the_run_in_progress_and_prints_it() {
    stops_after_the_run_in_progress(&Stop::Shutdown);
}
