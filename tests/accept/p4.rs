//! SPEC 100 acceptance (POC 4) on the mock (P4-8). Three checks gate the run:
//! - the three P4 workloads meet P4-2 and P4-3;
//! - every knob changes the requests of some workload;
//! - a reduced L1 loop (`tests/accept/fixtures/p4t-timestamp.toml`) shows the
//!   mock's model of caching reacting as built, with the shipped default as its
//!   control (CON-18).
//!
//! It also checks that `hypotheses/p4.toml` is accepted at the run of record's
//! budget (P4-7). Everything here is `mock-gated` and never cited (CON-26).

#![allow(clippy::unwrap_used, clippy::expect_used, clippy::panic)] // CON-19: tests are exempt

use std::collections::BTreeMap;
use std::path::{Path, PathBuf};

use acn_accept::build;
use acn_cli::loop_exec::HarnessExecutor;
use acn_harness::agent::Opts;
use acn_harness::run::{HypothesisArg, RunConfig, run};
use acn_harness::wire::Backend;
use acn_harness::workload::Workload;
use acn_hyp::loop_run::{self, Args, Binary, Code, Executor, Request};
use acn_hyp::read::BundleData;
use acn_trace::identity::{Digest, Mode};

const WORKLOADS: [(&str, &str); 3] = [
    ("coding", "p4-coding.toml"),
    ("retrieval", "p4-retrieval.toml"),
    ("fanout", "p4-fanout.toml"),
];

/// The mock profiles of P4-5 for the two priced providers.
const PROFILES: [(&str, &str); 2] = [("anthropic", "mock-explicit"), ("openai", "mock-auto")];

fn root() -> PathBuf {
    std::fs::canonicalize(concat!(env!("CARGO_MANIFEST_DIR"), "/../..")).unwrap()
}

fn workload(file: &str) -> PathBuf {
    root().join("workloads").join(file)
}

fn engine() -> Digest {
    Digest::of(b"engine")
}

/// One replicate of `file` on `model`, with `vary` knob values, in a fresh
/// directory outside any workspace; the embedded profiles, as `acn` ships them.
fn one(file: &str, model: &str, vary: &[(&str, &str)]) -> BundleData {
    let dir = tempfile::tempdir().unwrap();
    let w = run(&RunConfig {
        workload: workload(file),
        backend: Backend::Mockllm,
        model: model.into(),
        mode: Mode::Sim,
        arm: "treatment".into(),
        replicates: 1,
        vary: vary
            .iter()
            .map(|(k, v)| ((*k).to_owned(), (*v).to_owned()))
            .collect::<BTreeMap<_, _>>(),
        opts: Opts::default(),
        hypothesis: HypothesisArg::None { seed: 7 },
        runs_dir: dir.path().join("runs"),
        start_dir: dir.path().to_path_buf(),
        engine_hash: engine(),
        build: build("p4"),
        profiles: None,
    })
    .unwrap();
    acn_hyp::read::read(&w.dir).unwrap()
}

/// One call's input, cache-read, cache-write and output tokens.
type Counts = (Option<i64>, Option<i64>, Option<i64>, Option<i64>);

/// What the mock accounted for each call, in order: it differs only when the
/// requests differ, since the seed and the mock are the same.
fn accounting(b: &BundleData) -> Vec<Counts> {
    b.calls
        .iter()
        .map(|c| {
            (
                c.input_tokens,
                c.cache_read_tokens,
                c.cache_write_tokens,
                c.output_tokens,
            )
        })
        .collect()
}

/// Every session (one per task, HAR-30) has a compacted turn.
fn every_task_compacts(b: &BundleData) -> bool {
    b.sessions.iter().all(|s| {
        b.turns
            .iter()
            .any(|t| t.session_id == s.session_id && t.compaction != "none")
    })
}

/// Cites: P4-1, P4-2, P4-3, P4-8
#[test]
fn the_p4_workloads_meet_the_spec() {
    for (value, file) in WORKLOADS {
        let w = Workload::load(&workload(file)).unwrap();
        // P4-2: the stable prefix is the system prompt, as written in the file
        // (the marker and the timestamp are added by the harness, HAR-11, HAR-42).
        let prefix = w.agent.system_prompt.len().div_ceil(4);
        assert!(prefix >= 2048, "{value}: {prefix} estimated tokens");
        // P4-3: the shape every workload shares.
        assert!(w.tasks.len() >= 2, "{value}");
        assert!(w.tasks.iter().all(|t| t.turns.len() >= 3), "{value}");
        assert!(w.tools.len() >= 3, "{value}");
        for tool in &w.tools {
            assert!(
                w.tasks.iter().any(|t| t.tools.first() == Some(&tool.name)),
                "{value}: `{}` comes first in no task (MLM-40)",
                tool.name
            );
            if let Some(r) = tool.result_bytes {
                assert!(r.max > r.min, "{value}: `{}`", tool.name);
            }
        }
        assert!(
            w.tasks
                .iter()
                .flat_map(|t| &t.turns)
                .all(|t| t.expect_tools.is_empty()),
            "{value}: success must not depend on tool choice"
        );
        assert!(w.agent.max_tokens >= 256 && w.agent.summary_max_tokens >= 256);
        let updates = w
            .tasks
            .iter()
            .flat_map(|t| &t.turns)
            .any(|t| !t.updates.is_empty());
        match value {
            "coding" => {
                assert!(w.tool("read_file").is_some() && w.tool("grep").is_some());
                assert!(updates);
            }
            "retrieval" => {
                assert!(
                    w.tools
                        .iter()
                        .any(|t| t.result_bytes.is_some_and(|r| r.min >= 4000))
                );
                assert!(updates);
            }
            _ => assert!(
                w.tools
                    .iter()
                    .any(|t| t.is_subagent() && t.width.is_some_and(|n| n >= 3))
            ),
        }
        // Every turn completes on both priced profiles (SPEC 100 §1).
        for (_, model) in PROFILES {
            let b = one(file, model, &[]);
            assert!(
                b.turns.iter().all(|t| t.outcome == "success"),
                "{value} on {model}"
            );
        }
    }
    // P4-3, `coding`: every task compacts under each trigger, on both profiles.
    for (_, model) in PROFILES {
        assert!(
            every_task_compacts(&one("p4-coding.toml", model, &[])),
            "window_full on {model}"
        );
        assert!(
            every_task_compacts(&one(
                "p4-coding.toml",
                model,
                &[
                    ("compaction_trigger", "read_cost_threshold"),
                    ("timestamp_in_system_prompt", "true"),
                ],
            )),
            "read_cost_threshold on {model}"
        );
    }
}

/// Cites: P4-3, P4-8
#[test]
fn every_knob_changes_the_requests_of_some_p4_workload() {
    // Each knob moved off its shipped default (HAR-10).
    let moved = [
        ("timestamp_in_system_prompt", "false"),
        ("tool_order_stable", "false"),
        ("backfill_mode", "tail_restate"),
        ("fanout_prompting", "fork_from_prefix"),
        ("compaction_trigger", "read_cost_threshold"),
        ("cache_breakpoint_placement", "none"),
    ];
    let mut control = BTreeMap::new();
    for (_, file) in WORKLOADS {
        for (_, model) in PROFILES {
            control.insert((file, model), accounting(&one(file, model, &[])));
        }
    }
    for (knob, value) in moved {
        let changes: Vec<String> = WORKLOADS
            .iter()
            .flat_map(|(w, file)| PROFILES.iter().map(move |(_, m)| (*w, *file, *m)))
            .filter(|(_, file, model)| {
                accounting(&one(file, model, &[(knob, value)])) != control[&(*file, *model)]
            })
            .map(|(w, _, m)| format!("{w} on {m}"))
            .collect();
        assert!(!changes.is_empty(), "`{knob}` changes no P4 workload");
    }
}

/// `acn`'s executor with this suite's build identity.
fn harness(tag: &str) -> (HarnessExecutor, Binary) {
    let x = HarnessExecutor {
        engine_hash: engine(),
        build: build(tag),
    };
    let bin = Binary {
        engine_hash: engine(),
        build_hash: Digest::from_hex(&x.build.build_hash).unwrap(),
    };
    (x, bin)
}

fn models() -> Vec<String> {
    PROFILES.iter().map(|(p, m)| format!("{p}={m}")).collect()
}

/// Cites: P4-5, P4-8, CON-18
#[test]
fn the_mock_reacts_to_a_timestamp_in_the_system_prompt_as_built() {
    let dir = tempfile::tempdir().unwrap();
    let d = dir.path();
    std::fs::copy(
        root().join("tests/accept/fixtures/p4t-timestamp.toml"),
        d.join("p4t-timestamp.toml"),
    )
    .unwrap();
    std::fs::copy(workload("p4-coding.toml"), d.join("p4-coding.toml")).unwrap();
    let h = acn_hyp::load_in(&d.join("p4t-timestamp.toml"), d).unwrap();
    let (mut x, bin) = harness("p4");
    let args = Args {
        workloads: vec![d.join("p4-coding.toml").display().to_string()],
        models: models(),
        budget: 10,
    };
    let c = loop_run::run(&h, &args, &d.join("runs"), bin, &mut x).unwrap();
    assert_eq!(c.run_ids.len(), 6, "two cells and a control per provider");
    // The report regenerates (LOOP-14).
    assert!(
        loop_run::regenerate(&c.report, bin, &mut x)
            .unwrap()
            .identical()
    );
    // The verdict: mock-gated, exploratory, a pass in each provider's slice.
    let v: serde_json::Value = serde_json::from_str(
        &std::fs::read_to_string(
            d.join("runs/verdicts")
                .join(c.verdict_id.to_hex())
                .join("verdict.json"),
        )
        .unwrap(),
    )
    .unwrap();
    let labels: Vec<&str> = v["labels"]
        .as_array()
        .unwrap()
        .iter()
        .map(|l| l.as_str().unwrap())
        .collect();
    assert!(labels.contains(&"mock-gated") && labels.contains(&"exploratory"));
    let slices: Vec<(&str, &str)> = v["slices"]
        .as_array()
        .unwrap()
        .iter()
        .map(|s| (s["key"].as_str().unwrap(), s["verdict"].as_str().unwrap()))
        .collect();
    assert_eq!(
        slices,
        [("provider=anthropic", "pass"), ("provider=openai", "pass")]
    );
    // The mechanism: without the timestamp, cost per completed turn falls, and
    // its 95% interval lies below zero on both providers.
    let r: serde_json::Value =
        serde_json::from_str(&std::fs::read_to_string(&c.report).unwrap()).unwrap();
    for p in ["anthropic", "openai"] {
        let cell = format!("provider={p},timestamp_in_system_prompt=false");
        let e = r["control_effect"]
            .as_array()
            .unwrap()
            .iter()
            .find(|e| e["cell"] == cell && e["quantity"] == "cost_per_success")
            .unwrap();
        assert!(e["ci_high"].as_f64().unwrap() < 0.0, "{p}: {e}");
        // CON-18: the effect comes with its replicate counts.
        assert_eq!(e["treatment_replicates"], 20, "{e}");
        assert_eq!(e["control_replicates"], 20, "{e}");
    }
    // The control is the harness's shipped default (HAR-10): only the
    // timestamp knob and the provider are set on its bundles.
    for id in &c.run_ids {
        let m = acn_trace::bundle::verify(&d.join("runs").join(id.to_hex()))
            .unwrap()
            .manifest;
        if m.params["arms"] == "control" {
            assert_eq!(m.params["vary.timestamp_in_system_prompt"], "true");
            let knobs: Vec<&String> = m.params.keys().filter(|k| k.starts_with("vary.")).collect();
            assert_eq!(knobs.len(), 2, "{knobs:?}");
        }
    }
}

/// An executor that refuses every run: reaching it means every check before
/// the first batch passed.
struct NoRuns(HarnessExecutor);

impl Executor for NoRuns {
    fn run(&mut self, _: &Request) -> Result<PathBuf, String> {
        Err("this test runs nothing".into())
    }
    fn check_model(&self, model: &str) -> Result<(), String> {
        self.0.check_model(model)
    }
    fn check_workload(&self, path: &Path) -> Result<(), String> {
        self.0.check_workload(path)
    }
}

/// Cites: P4-7, P4-1, P4-5
#[test]
fn the_run_of_record_is_accepted_at_its_budget_and_refused_below_it() {
    let r = root();
    let h = acn_hyp::load_in(&r.join("hypotheses/p4.toml"), &r).unwrap();
    assert_eq!(h.status(), acn_hyp::Status::Frozen);
    let (x, bin) = harness("p4");
    let mut x = NoRuns(x);
    let args = |budget| Args {
        workloads: WORKLOADS
            .iter()
            .map(|(v, f)| format!("{v}=workloads/{f}"))
            .collect(),
        models: [
            "anthropic=mock-explicit",
            "openai=mock-auto",
            "vllm=mock-blocks",
            "sglang=mock-blocks",
        ]
        .map(String::from)
        .to_vec(),
        budget,
    };
    // Nothing is written either way: the loop stops before its first bundle.
    let runs = r.join("runs");
    let e = loop_run::run(&h, &args(1547), &runs, bin, &mut x).unwrap_err();
    assert_eq!(e.code, Code::BudgetTooSmall, "{e}");
    assert!(e.message.contains("1548 bundles"), "{e}");
    let e = loop_run::run(&h, &args(1548), &runs, bin, &mut x).unwrap_err();
    assert_eq!(e.code, Code::ExecutorFailed, "every check passed: {e}");
}
