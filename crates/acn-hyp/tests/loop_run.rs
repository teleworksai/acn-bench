//! LOOP-10, LOOP-11, LOOP-15 (SPEC 085 §2): `acn loop run` on real harness
//! bundles in `sim`: each strategy's cells and stop rule, the budget, every
//! refusal before anything runs and every abort, the trajectory, the final
//! verdict and the rule for one that exists, the report's fields, the known
//! answers for `loop_id` and `random`, bundle reuse within a build, and the
//! layer of each kind of object (LOOP-1).

#![allow(clippy::unwrap_used, clippy::expect_used, clippy::panic)] // CON-19: tests are exempt

mod common;

use std::collections::{BTreeMap, BTreeSet};
use std::path::Path;

use acn_hyp::layer::{self, Layer};
use acn_hyp::loop_run::{self, Args, Code, Stop};
use acn_hyp::verdict::{Role, verdict};
use acn_trace::identity::Digest;
use common::exec::{Exec, TWO, args, args_in, bundles, dir_with, report, run_loop, smoke};

fn code(r: Result<loop_run::Completed, loop_run::LoopError>) -> Code {
    match r {
        Ok(c) => panic!("completed: {c:?}"),
        Err(e) => e.code,
    }
}

/// Cites: LOOP-10, LOOP-11, LOOP-15
#[test]
fn a_grid_loop_runs_every_cell_in_order_and_writes_its_report() {
    let dir = dir_with(TWO);
    let d = dir.path();
    let mut ex = Exec::new(d);
    let c = run_loop(d, args(10), &mut ex).unwrap();
    assert_eq!(c.stop, Stop::Exhausted);
    assert_eq!(c.run_ids.len(), 3);
    // The grid in HYP-14 order, each cell's treatment then its control once.
    let asked: Vec<(&str, &str)> = ex
        .requests
        .iter()
        .map(|r| (r.arm.as_str(), r.vary["tool_order_stable"].as_str()))
        .collect();
    assert_eq!(
        asked,
        [
            ("treatment", "false"),
            ("control", "true"),
            ("treatment", "true")
        ]
    );
    // The executor is asked exactly HAR-50's inputs (LOOP-15).
    for r in &ex.requests {
        assert_eq!((r.model.as_str(), r.replicates), ("mock-auto", 4));
        assert_eq!(r.runs_dir, d.join("runs"));
        assert!(r.workload.ends_with("w.toml") && r.hypothesis.ends_with("zz.toml"));
    }

    let r = report(&c);
    assert_eq!(r["format"], "acn-bench/loop-report/v1");
    assert_eq!(r["layer"], "L1");
    assert_eq!(r["loop_id"], c.loop_id.to_hex());
    assert_eq!(r["hypothesis"]["id"], "zz");
    assert_eq!(r["hypothesis"]["status"], "candidate");
    assert_eq!(
        r["hypothesis"]["path"], "zz.toml",
        "relative to the runs/ parent"
    );
    assert_eq!(r["inputs"]["workloads"][""]["path"], "w.toml");
    assert_eq!(
        r["inputs"]["workloads"][""]["hash"],
        Digest::of(smoke().as_bytes()).to_hex()
    );
    assert_eq!(r["inputs"]["models"][""], "mock-auto");
    assert_eq!(r["inputs"]["strategy"], "grid");
    assert_eq!(r["inputs"]["budget"], 10);
    assert_eq!(r["build_hash"], ex.bin().build_hash.to_hex());
    assert_eq!(r["engine_hash"], ex.bin().engine_hash.to_hex());
    let h = acn_hyp::load_in(&d.join("zz.toml"), d).unwrap();
    assert_eq!(
        r["seed"],
        acn_hyp::verdict::hypothesis_seed(&h.hash())
            .unwrap()
            .to_string()
    );
    assert_eq!(r["stop"], "exhausted");
    let batches = r["batches"].as_array().unwrap();
    assert_eq!(batches.len(), 2);
    assert_eq!(batches[0]["cell"]["tool_order_stable"], "false");
    assert_eq!(batches[0]["run_ids"].as_array().unwrap().len(), 2);
    assert_eq!(batches[1]["run_ids"].as_array().unwrap().len(), 1);
    // The trajectory: one grid cell missing after the first batch; the last
    // entry is the final verdict.
    assert_eq!(batches[0]["verdict"], "inconclusive");
    assert_eq!(batches[1]["verdict"], r["verdict"]);
    assert_eq!(r["verdict_id"], c.verdict_id.to_hex());
    let listed: Vec<&str> = r["bundles"]
        .as_array()
        .unwrap()
        .iter()
        .map(|b| b["run_id"].as_str().unwrap())
        .collect();
    let returned: Vec<String> = c.run_ids.iter().map(Digest::to_hex).collect();
    assert_eq!(listed, returned, "every bundle, ascending");
    assert_eq!(
        bundles(d).keys().cloned().collect::<Vec<_>>(),
        returned,
        "and nothing else"
    );
    assert_eq!(r["control_effect"].as_array().unwrap().len(), 2);
    assert_eq!(r["best"]["quantity"], "cached_token_ratio");
    assert!(r["worst"]["effect"].is_number());
    assert_eq!(r["lab_note"]["question"], h.statement());
    assert_eq!(
        r["lab_note"]["varied"]["tool_order_stable"],
        serde_json::json!(["false", "true"])
    );
    assert_eq!(r["lab_note"]["next_layer"], "L3", "twin_required = false");

    // The final verdict is written once, and is what `acn hyp verdict` makes of
    // the same bundles (HYP-20).
    let vpath = d
        .join("runs/verdicts")
        .join(c.verdict_id.to_hex())
        .join("verdict.json");
    let set = bundles(d)
        .values()
        .map(|p| acn_hyp::read::read(p).unwrap())
        .collect();
    let v = verdict(&h, set, ex.engine).unwrap();
    assert_eq!(std::fs::read_to_string(vpath).unwrap(), v.text());
    assert_eq!(c.verdict, v.verdict);
    // report.md is made from report.json alone.
    let json = std::fs::read_to_string(&c.report).unwrap();
    assert_eq!(
        std::fs::read_to_string(c.report.with_file_name("report.md")).unwrap(),
        loop_run::markdown(&json).unwrap()
    );
    assert!(
        json.ends_with("}\n") && !json.contains(": "),
        "canonical JSON"
    );

    // A report is never overwritten, and a refused loop runs nothing.
    let mut again = Exec::new(d);
    assert_eq!(code(run_loop(d, args(10), &mut again)), Code::LoopExists);
    assert!(again.requests.is_empty());
}

/// Cites: LOOP-10, LOOP-11
#[test]
fn the_budget_stops_the_loop_at_the_first_batch_that_does_not_fit() {
    let dir = dir_with(TWO);
    let d = dir.path();
    // 1: smaller than the first batch (a treatment and its control).
    let mut ex = Exec::new(d);
    assert_eq!(code(run_loop(d, args(1), &mut ex)), Code::BudgetTooSmall);
    assert!(ex.requests.is_empty() && bundles(d).is_empty());
    // 2: one batch; the second would need a third bundle.
    let c = run_loop(d, args(2), &mut ex).unwrap();
    assert_eq!(c.stop, Stop::Budget);
    assert_eq!(report(&c)["batches"].as_array().unwrap().len(), 1);
    assert_eq!(ex.requests.len(), 2);
    // 3: the two bundles above are reused (and counted), one is made.
    let mut ex3 = Exec::new(d);
    let c3 = run_loop(d, args(3), &mut ex3).unwrap();
    assert_eq!(c3.stop, Stop::Exhausted);
    assert_eq!(ex3.requests.len(), 1, "only the missing cell runs");
    assert_eq!(c3.run_ids.len(), 3);
    // The report does not say what was reused: a fresh directory gives the
    // same bytes.
    let fresh = dir_with(TWO);
    let mut exf = Exec::new(fresh.path());
    let cf = run_loop(fresh.path(), args(3), &mut exf).unwrap();
    assert_eq!(exf.requests.len(), 3);
    assert_eq!(
        std::fs::read(&c3.report).unwrap(),
        std::fs::read(&cf.report).unwrap()
    );
}

/// The frozen form of TWO under a temporary workspace root (HYP-3).
fn frozen_two() -> tempfile::TempDir {
    let text = TWO
        .replace(
            "title = \"tool order and the cache\"",
            "title = \"tool order and the cache\"\nspec = \"specs/100-x.md\"",
        )
        .replace("replicates = 4", "replicates = 20")
        .replace("replicates < 4", "replicates < 20");
    common::root(&[("hypotheses/zz.toml", &text), ("w.toml", &smoke())])
}

/// Cites: LOOP-10
#[test]
fn every_refusal_comes_before_anything_runs() {
    let refused = |text: &str, a: Args| -> Code {
        let dir = dir_with(text);
        let mut ex = Exec::new(dir.path());
        let c = code(run_loop(dir.path(), a, &mut ex));
        assert!(ex.requests.is_empty(), "{c:?}: nothing ran");
        assert!(!dir.path().join("runs").exists(), "{c:?}: nothing written");
        c
    };
    let with = |a: &str, b: &str| TWO.replace(a, b);
    assert_eq!(
        refused(&with("search = \"grid\"", "search = \"bisect\""), args(10)),
        Code::Search,
        "bisect is not specified yet"
    );
    // HYP-8 lets a candidate leave out the control's config.
    let no_control = with("config = { tool_order_stable = true }\n", "");
    assert_eq!(refused(&no_control, args(10)), Code::NoControl);
    assert_eq!(
        refused(
            &with(
                "config = { tool_order_stable = true }",
                "workload = \"plain_rpc\""
            ),
            args(10)
        ),
        Code::WorkloadControl
    );
    assert_eq!(
        refused(
            &with(
                "twin_required = false",
                "twin_required = false\nbackends = [\"real-api\"]"
            ),
            args(10)
        ),
        Code::Backends
    );
    let mut a = args(10);
    a.models = vec!["mock-nope".into()];
    assert_eq!(refused(TWO, a), Code::UnknownModel);
    let mut a = args(10);
    a.models = vec!["mock-auto".into(), "mock-explicit".into()];
    assert_eq!(refused(TWO, a), Code::BadMap, "two models, no provider");
    let mut a = args(10);
    a.workloads = vec!["missing.toml".into()];
    assert_eq!(refused(TWO, a), Code::Workload);
    let mut a = args(10);
    a.workloads = vec!["zz.toml".into()];
    assert_eq!(refused(TWO, a), Code::Workload, "not a workload");
    assert_eq!(refused(TWO, args(1)), Code::BudgetTooSmall);

    // A workload outside the directory runs/ lies in cannot be recorded.
    let dir = dir_with(TWO);
    let elsewhere = dir_with(TWO);
    let mut a = args(10);
    a.workloads = vec![elsewhere.path().join("w.toml").display().to_string()];
    let h = acn_hyp::load_in(&dir.path().join("zz.toml"), dir.path()).unwrap();
    let mut ex = Exec::new(dir.path());
    let bin = ex.bin();
    let e = loop_run::run(&h, &a, &dir.path().join("runs"), bin, &mut ex).unwrap_err();
    assert_eq!(e.code, Code::Path, "{e}");
    // Only a directory named `runs` takes a loop (HYP-4).
    let e = loop_run::run(
        &h,
        &args_in(dir.path(), args(10)),
        &dir.path().join("out"),
        bin,
        &mut ex,
    )
    .unwrap_err();
    assert_eq!(e.code, Code::Path, "{e}");
    assert!(ex.requests.is_empty());

    // Pins: every check of HYP-20 that can be decided before running.
    let provider = "tool_order_stable = { kind = \"bool\" }\nprovider = { kind = \"enum\", values = [\"p1\"] }";
    let pinned = |scenario: &str, workload: &str, model: &str| {
        with("tool_order_stable = { kind = \"bool\" }", provider).replace(
            "twin_required = false",
            &format!(
                "twin_required = false\npins = {{ scenario = [\"{scenario}\"], workload = [\"{workload}\"], models = {{ p1 = \"{model}\" }} }}"
            ),
        )
    };
    let zero = Digest::ZERO.to_hex();
    let w = Digest::of(smoke().as_bytes()).to_hex();
    let other = Digest::of(b"other").to_hex();
    assert_eq!(
        refused(&pinned(&zero, &other, "mock-auto"), args(10)),
        Code::Pins
    );
    assert_eq!(
        refused(&pinned(&other, &w, "mock-auto"), args(10)),
        Code::Pins
    );
    assert_eq!(
        refused(&pinned(&zero, &w, "mock-explicit"), args(10)),
        Code::Pins
    );
    // Pinned exactly as the loop runs, it runs.
    let dir = dir_with(&pinned(&zero, &w, "mock-auto"));
    let mut ex = Exec::new(dir.path());
    run_loop(dir.path(), args(2), &mut ex).unwrap();

    // A frozen file runs only as a grid (its load refuses any other search,
    // HYP-9), with a budget that covers the grid: here 3 bundles.
    let root = frozen_two();
    let r = root.path();
    let h = acn_hyp::load_in(&r.join("hypotheses/zz.toml"), r).unwrap();
    assert_eq!(h.status(), acn_hyp::Status::Frozen);
    let mut ex = Exec::new(r);
    let bin = ex.bin();
    let e = loop_run::run(&h, &args_in(r, args(2)), &r.join("runs"), bin, &mut ex).unwrap_err();
    assert_eq!(e.code, Code::BudgetTooSmall, "{e}");
    assert!(e.message.contains("3 bundles"), "{e}");
    assert!(ex.requests.is_empty());
}

/// Cites: LOOP-11
#[test]
fn an_existing_bundle_is_reused_on_its_build_and_refused_from_another() {
    let dir = dir_with(TWO);
    let d = dir.path();
    run_loop(d, args(2), &mut Exec::with_build(d, "a")).unwrap();
    let before = bundles(d);
    let mut b = Exec::with_build(d, "b");
    let e = run_loop(d, args(3), &mut b).unwrap_err();
    assert_eq!(e.code, Code::BuildMismatch, "{e}");
    assert!(e.message.contains("build"), "{e}");
    assert!(
        b.requests.is_empty(),
        "refused before the batch that would use it"
    );
    assert_eq!(bundles(d), before);
    assert_eq!(
        std::fs::read_dir(d.join("runs/loop")).unwrap().count(),
        1,
        "no report for the aborted loop"
    );
    // On build `a` the same loop reuses them.
    let mut a = Exec::with_build(d, "a");
    run_loop(d, args(3), &mut a).unwrap();
    assert_eq!(a.requests.len(), 1);
}

/// Cites: LOOP-15, LOOP-10
#[test]
fn an_executor_that_fails_or_returns_another_bundle_aborts_the_loop() {
    let aborted = |ex: Exec, want: Code| {
        let dir = dir_with(TWO);
        let d = dir.path();
        let mut ex = Exec {
            start: d.to_path_buf(),
            ..ex
        };
        let e = run_loop(d, args(10), &mut ex).unwrap_err();
        assert_eq!(e.code, want, "{e}");
        assert!(!d.join("runs/loop").exists(), "no report");
        assert!(!d.join("runs/verdicts").exists(), "no verdict");
        assert!(!bundles(d).is_empty(), "the bundles made stay");
    };
    let start = Path::new(".");
    aborted(
        Exec::new(start).hook(|n, _, dir| if n == 1 { Err("boom".into()) } else { Ok(dir) }),
        Code::ExecutorFailed,
    );
    // The control's request answered with the treatment's bundle.
    let mut first = None;
    aborted(
        Exec::new(start).hook(move |n, _, dir| {
            if n == 0 {
                first = Some(dir.clone());
                Ok(dir)
            } else {
                Ok(first.clone().unwrap())
            }
        }),
        Code::ExecutorMismatch,
    );
    // A directory that is not a bundle.
    aborted(
        Exec::new(start).hook(|_, _, dir| Ok(dir.join("logs"))),
        Code::ExecutorMismatch,
    );
}

/// Cites: LOOP-10
#[test]
fn two_loops_that_end_on_one_bundle_set_share_its_verdict() {
    let dir = dir_with(TWO);
    let d = dir.path();
    let a = run_loop(d, args(10), &mut Exec::new(d)).unwrap();
    let mut ex = Exec::new(d);
    let b = run_loop(d, args(20), &mut ex).unwrap();
    assert_ne!(a.loop_id, b.loop_id);
    assert_eq!(a.verdict_id, b.verdict_id, "the same set, the same verdict");
    assert!(ex.requests.is_empty());
    // Other bytes under that verdict_id are a conflict, and nothing is written.
    let vpath = d
        .join("runs/verdicts")
        .join(a.verdict_id.to_hex())
        .join("verdict.json");
    let mut text = std::fs::read_to_string(&vpath).unwrap();
    text.push(' ');
    std::fs::write(&vpath, text).unwrap();
    let e = run_loop(d, args(30), &mut Exec::new(d)).unwrap_err();
    assert_eq!(e.code, Code::VerdictConflict, "{e}");
    assert_eq!(std::fs::read_dir(d.join("runs/loop")).unwrap().count(), 2);
}

/// Cites: LOOP-10, LOOP-11
#[test]
fn a_workload_parameter_runs_each_value_on_its_own_file() {
    let text = TWO.replace(
        "tool_order_stable = { kind = \"bool\" }",
        "tool_order_stable = { kind = \"bool\" }\nworkload = { kind = \"enum\", values = [\"a\", \"b\"] }",
    );
    let dir = dir_with(&text);
    let d = dir.path();
    let w2 = format!("{}\n# the second workload\n", smoke());
    std::fs::write(d.join("w2.toml"), &w2).unwrap();
    let hashes = BTreeMap::from([
        ("a".to_owned(), Digest::of(smoke().as_bytes()).to_hex()),
        ("b".to_owned(), Digest::of(w2.as_bytes()).to_hex()),
    ]);
    // One file for a file that varies `workload`, or a partial map: refused.
    for ws in [
        vec!["w.toml"],
        vec!["a=w.toml"],
        vec!["a=w.toml", "b=w2.toml", "c=w.toml"],
    ] {
        let mut a = args(20);
        a.workloads = ws.iter().map(|s| (*s).to_owned()).collect();
        let e = run_loop(d, a, &mut Exec::new(d)).unwrap_err();
        assert_eq!(e.code, Code::BadMap, "{ws:?}: {e}");
    }
    let mut a = args(20);
    a.workloads = vec!["a=w.toml".into(), "b=w2.toml".into()];
    let c = run_loop(d, a, &mut Exec::new(d)).unwrap();
    assert_eq!(c.stop, Stop::Exhausted);
    assert_eq!(c.run_ids.len(), 6, "four cells, a control per workload");
    for p in bundles(d).values() {
        let m = acn_trace::bundle::verify(p).unwrap().manifest;
        assert_eq!(m.workload_hash, hashes[&m.params["vary.workload"]]);
    }
    let r = report(&c);
    assert_eq!(r["inputs"]["workloads"]["b"]["path"], "w2.toml");
    assert_eq!(r["inputs"]["workloads"]["a"]["hash"], hashes["a"]);
}

/// Cites: LOOP-10
#[test]
fn a_provider_parameter_may_run_each_value_on_its_own_profile() {
    let text = TWO.replace(
        "tool_order_stable = { kind = \"bool\" }",
        "tool_order_stable = { kind = \"bool\" }\nprovider = { kind = \"enum\", values = [\"p1\", \"p2\"] }",
    );
    let dir = dir_with(&text);
    let d = dir.path();
    let mut a = args(20);
    a.models = vec!["p1=mock-auto".into(), "p2=mock-explicit".into()];
    let c = run_loop(d, a, &mut Exec::new(d)).unwrap();
    assert_eq!(
        c.run_ids.len(),
        6,
        "two slices of two cells, a control each"
    );
    for p in bundles(d).values() {
        let m = acn_trace::bundle::verify(p).unwrap().manifest;
        let want = if m.params["vary.provider"] == "p1" {
            "mock-auto"
        } else {
            "mock-explicit"
        };
        assert_eq!(m.model, want);
    }
    assert_eq!(report(&c)["inputs"]["models"]["p2"], "mock-explicit");
    // One profile for every provider is still allowed.
    let dir = dir_with(&text);
    run_loop(dir.path(), args(2), &mut Exec::new(dir.path())).unwrap();
}

const RANDOM: &str =
    "tool_order_stable = { kind = \"bool\" }\nn = { kind = \"int_range\", min = 0, max = 2 }";

/// Cites: LOOP-10, LOOP-14
#[test]
fn random_draws_from_its_sub_stream_and_is_exhausted_by_redraws() {
    let text = TWO
        .replace("tool_order_stable = { kind = \"bool\" }", RANDOM)
        .replace("search = \"grid\"", "search = \"random\"");
    let dir = dir_with(&text);
    let d = dir.path();
    let mut ex = Exec::new(d);
    let c = run_loop(d, args(100), &mut ex).unwrap();
    assert_eq!(c.stop, Stop::Exhausted, "six cells, then 1 000 redraws");
    let treatments: BTreeSet<(String, String)> = ex
        .requests
        .iter()
        .filter(|r| r.arm == Role::Treatment)
        .map(|r| (r.vary["tool_order_stable"].clone(), r.vary["n"].clone()))
        .collect();
    assert_eq!(treatments.len(), 6, "each cell once");
    assert_eq!(c.run_ids.len(), 9, "six treatments, a control per n");
    // The same file draws the same cells: the reports of two directories agree.
    let again = dir_with(&text);
    let c2 = run_loop(again.path(), args(5), &mut Exec::new(again.path())).unwrap();
    let again2 = dir_with(&text);
    let c3 = run_loop(again2.path(), args(5), &mut Exec::new(again2.path())).unwrap();
    assert_eq!(c2.stop, Stop::Budget);
    assert_eq!(
        std::fs::read(&c2.report).unwrap(),
        std::fs::read(&c3.report).unwrap()
    );
}

/// Cites: LOOP-10, LOOP-14, CON-27
#[test]
fn random_draws_have_a_known_answer() {
    let text = TWO
        .replace(
            "tool_order_stable = { kind = \"bool\" }",
            "a = { kind = \"enum\", values = [\"x\", \"y\", \"z\"] }\nb = { kind = \"range\", min = 0.5, max = 2.5 }\nc = { kind = \"int_range\", min = -3, max = 3 }\ntool_order_stable = { kind = \"bool\" }",
        )
        .replace("search = \"grid\"", "search = \"random\"\nseed = 7");
    let (h, _dir) = common::candidate(&text, "zz");
    let h = h.unwrap();
    let keys: Vec<String> = loop_run::random_draws(&h, 4)
        .unwrap()
        .iter()
        .map(acn_hyp::slice::key)
        .collect();
    assert_eq!(keys, KNOWN_DRAWS);
}

const KNOWN_DRAWS: [&str; 4] = [
    "a=x,b=2.1934981865701655,c=-2,tool_order_stable=true",
    "a=z,b=1.775657718866563,c=1,tool_order_stable=false",
    "a=x,b=1.499193026399342,c=-3,tool_order_stable=true",
    "a=z,b=1.2415933373844987,c=-1,tool_order_stable=false",
];

/// Cites: LOOP-11, CON-27
#[test]
fn loop_id_has_a_known_answer() {
    let one = loop_run::loop_id(
        &Digest::of(b"hypothesis"),
        &Digest::of(b"engine"),
        "candidate",
        &BTreeMap::from([(String::new(), Digest::of(b"workload"))]),
        &BTreeMap::from([(String::new(), "mock-auto".to_owned())]),
        "grid",
        10,
    )
    .unwrap();
    let mapped = loop_run::loop_id(
        &Digest::of(b"hypothesis"),
        &Digest::of(b"engine"),
        "frozen",
        &BTreeMap::from([
            ("a".to_owned(), Digest::of(b"wa")),
            ("b".to_owned(), Digest::of(b"wb")),
        ]),
        &BTreeMap::from([
            ("p1".to_owned(), "mock-auto".to_owned()),
            ("p2".to_owned(), "mock-explicit".to_owned()),
        ]),
        "grid",
        u64::MAX,
    )
    .unwrap();
    assert_eq!(one.to_hex(), KNOWN_LOOP_ID.0);
    // The preimage written out byte by byte (CON-27): context, digests, then
    // length-prefixed strings, u32 counts and a u64 budget, little-endian.
    let s = |b: &mut Vec<u8>, x: &str| {
        b.extend(u32::try_from(x.len()).unwrap().to_le_bytes());
        b.extend(x.as_bytes());
    };
    let mut b = b"acn-bench/loop_id/v1\0".to_vec();
    b.extend(blake3::hash(b"hypothesis").as_bytes());
    b.extend([0u8; 32]);
    b.extend(blake3::hash(b"engine").as_bytes());
    s(&mut b, "candidate");
    b.extend(1u32.to_le_bytes());
    s(&mut b, "");
    b.extend(blake3::hash(b"workload").as_bytes());
    b.extend(1u32.to_le_bytes());
    s(&mut b, "");
    s(&mut b, "mock-auto");
    s(&mut b, "grid");
    b.extend(10u64.to_le_bytes());
    assert_eq!(blake3::hash(&b).to_hex().to_string(), KNOWN_LOOP_ID.0);
    assert_eq!(mapped.to_hex(), KNOWN_LOOP_ID.1);
}

const KNOWN_LOOP_ID: (&str, &str) = (
    "2a0d2963c9af02b6481b71b384327527e68052d6217776ecdd1af98861f01640",
    "6d373a4684f3f8098638b7957e8fdebc7af242ad95095a7a02bfd4876ccada0f",
);

/// Cites: LOOP-1
#[test]
fn an_objects_layer_is_derived_from_what_it_records() {
    use common::bundles::{Spec, bundle, flat};
    let dir = dir_with(TWO);
    let d = dir.path();
    let c = run_loop(d, args(10), &mut Exec::new(d)).unwrap();
    let h = acn_hyp::load_in(&d.join("zz.toml"), d).unwrap();
    let set: Vec<_> = bundles(d)
        .values()
        .map(|p| acn_hyp::read::read(p).unwrap())
        .collect();
    for b in &set {
        assert_eq!(layer::of_bundle(&b.manifest).unwrap(), Layer::L1);
    }
    let v = verdict(&h, set, Exec::new(d).engine).unwrap();
    assert_eq!(layer::of_verdict(&v), Layer::L1);
    assert_eq!(report(&c)["layer"], layer::REPORT.as_str());
    // A manifest's mode and backend fix its layer.
    let m = |mode: &str, backend: &str| {
        let mut x = bundle(
            &h,
            &Spec::new(
                "m",
                &[("tool_order_stable", "true")],
                "treatment",
                flat(4, 0.5),
            ),
        )
        .manifest;
        x.mode = mode.into();
        x.backend = backend.into();
        layer::of_bundle(&x)
    };
    assert_eq!(m("live", "mockllm").unwrap(), Layer::L2);
    assert_eq!(m("live", "openai").unwrap(), Layer::L3);
    assert_eq!(m("netem", "mockllm").unwrap(), Layer::L3);
    assert_eq!(m("netem", "openai").unwrap(), Layer::L3);
    assert!(m("sim", "openai").is_err(), "sim runs only on the mock");
    // A verdict over sim bundles and their live twins is L2.
    let twin = |name: &str, mode: &str, arm: &str, v: &str| {
        bundle(
            &h,
            &Spec::new(name, &[("tool_order_stable", v)], arm, flat(4, 0.5)).mode(mode),
        )
    };
    let set = vec![
        twin("s1", "sim", "treatment", "false"),
        twin("s2", "sim", "treatment", "true"),
        twin("s3", "sim", "control", "true"),
        twin("l1", "live", "treatment", "false"),
        twin("l2", "live", "treatment", "true"),
        twin("l3", "live", "control", "true"),
    ];
    let v = verdict(&h, set, common::bundles::engine()).unwrap();
    assert_eq!(layer::of_verdict(&v), Layer::L2);
}
