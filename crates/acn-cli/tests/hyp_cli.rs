//! `acn hyp lint` (HYP-27) under the CLI contract (CON-8).

#![allow(clippy::unwrap_used, clippy::expect_used, clippy::panic)] // CON-19: tests are exempt

use std::path::Path;
use std::process::Command;

fn acn_in(dir: &Path, args: &[&str]) -> (Option<i32>, serde_json::Value) {
    let out = Command::new(env!("CARGO_BIN_EXE_acn"))
        .args(args)
        .current_dir(dir)
        .output()
        .unwrap();
    let text = String::from_utf8(out.stdout).unwrap();
    (
        out.status.code(),
        serde_json::from_str(text.trim()).unwrap(),
    )
}

fn acn(args: &[&str]) -> (Option<i32>, serde_json::Value) {
    acn_in(Path::new(env!("CARGO_MANIFEST_DIR")), args)
}

/// Cites: HYP-27, CON-8
#[test]
fn hyp_lint_prints_one_object_and_exits_by_its_verdict() {
    let p4 = concat!(env!("CARGO_MANIFEST_DIR"), "/../../hypotheses/p4.toml");
    let (code, json) = acn(&["hyp", "lint", p4]);
    assert_eq!(code, Some(0), "{json}");
    assert_eq!(json["ok"], true);
    assert_eq!(json["id"], "p4");
    assert_eq!(json["status"], "frozen");
    assert_eq!(
        json["predicate"],
        "(max_over_knobs(abs(effect(cost_per_success))) < noise_floor(cost_per_success, control, ci = 0.95))"
    );
    assert_eq!(
        json["guard"],
        "((replicates < 20) or (providers_reported < 2))"
    );
    assert!(json["warnings"].as_array().unwrap().is_empty(), "{json}");
    assert!(json["witness"]["fires"].is_object() && json["witness"]["holds"].is_object());
    // HYP-27: the parsed structure is printed.
    let s = &json["structure"];
    assert_eq!(
        s["measures"]["primary"][3]["name"], "cost_per_success",
        "{s}"
    );
    assert_eq!(s["measures"]["primary"][3]["unit"], "cost", "{s}");
    assert_eq!(s["params"].as_object().unwrap().len(), 8, "{s}");
    assert_eq!(s["params"]["provider"]["pooled"], false, "{s}");
    assert_eq!(s["control"]["config"]["tool_order_stable"], "true", "{s}");
    assert_eq!(s["design"]["replicates"], 20, "{s}");
    assert_eq!(s["expected"], "pass", "{s}");

    let (code, json) = acn(&["hyp", "lint", "/nonexistent/x.toml"]);
    assert_eq!(code, Some(1));
    assert_eq!(json["ok"], false);
    assert!(json["errors"][0].as_str().unwrap().contains("x.toml"));
}

/// Cites: HYP-27, CON-8
#[test]
fn a_frozen_file_that_can_never_fire_exits_one() {
    let p4 = std::fs::read_to_string(concat!(env!("CARGO_MANIFEST_DIR"), "/../../hypotheses/p4.toml"))
        .unwrap()
        .replace(
            "max_over_knobs(abs(effect(cost_per_success))) < noise_floor(cost_per_success, control, ci = 0.95)",
            "max_over_knobs(abs(effect(cost_per_success))) < 0",
        );
    let dir = tempfile::tempdir().unwrap();
    let r = dir.path();
    for d in acn_trace::env::FROZEN_SET {
        std::fs::create_dir_all(r.join(d)).unwrap();
    }
    std::fs::create_dir_all(r.join("specs")).unwrap();
    std::fs::write(
        r.join("specs/README.md"),
        "| 100-p4-harness-discipline.md |\n",
    )
    .unwrap();
    std::fs::write(r.join("hypotheses/p4.toml"), &p4).unwrap();
    let rec = acn_trace::env::compute(r).unwrap();
    std::fs::write(
        r.join("env-hash.json"),
        serde_json::to_string(&rec).unwrap(),
    )
    .unwrap();

    let (code, json) = acn_in(r, &["hyp", "lint", "hypotheses/p4.toml"]);
    assert_eq!(code, Some(1), "{json}");
    assert_eq!(json["ok"], false);
    assert_eq!(json["status"], "frozen");
    assert!(
        json["errors"][0]
            .as_str()
            .unwrap()
            .contains("can never fire"),
        "{json}"
    );

    // The same file outside a workspace root is a candidate: reported, exit 0.
    let loose = tempfile::tempdir().unwrap();
    std::fs::write(loose.path().join("p4.toml"), &p4).unwrap();
    let (code, json) = acn_in(loose.path(), &["hyp", "lint", "p4.toml"]);
    assert_eq!(code, Some(0), "{json}");
    assert_eq!(json["status"], "candidate");
    assert!(
        json["warnings"]
            .as_array()
            .unwrap()
            .iter()
            .any(|w| w.as_str().unwrap().contains("can never fire")),
        "{json}"
    );
}

const ZZ: &str = r#"[poc]
id = "zz"
title = "tool order and the cache"

[hypothesis]
statement = "A stable tool order moves the cached-token ratio."

[varies]
tool_order_stable = { kind = "bool" }

[measures]
primary = ["cached_token_ratio"]

[control]
description = "the shipped default"
config = { tool_order_stable = true }

[design]
search = "grid"
replicates = 4
twin_required = false

[falsifier]
predicate = "max_over_knobs(abs(effect(cached_token_ratio))) < 0.001"

[expected]
outcome = "pass"
"#;

/// Cites: HYP-20, CON-8
#[test]
fn hyp_verdict_judges_harness_bundles_and_never_overwrites_a_verdict() {
    let dir = tempfile::tempdir().unwrap();
    std::fs::write(dir.path().join("zz.toml"), ZZ).unwrap();
    let workload = concat!(
        env!("CARGO_MANIFEST_DIR"),
        "/../../workloads/harness-smoke.toml"
    );
    let mut bundles = Vec::new();
    for (arm, stable) in [
        ("treatment", "false"),
        ("treatment", "true"),
        ("control", "true"),
    ] {
        let (code, json) = acn_in(
            dir.path(),
            &[
                "harness",
                "run",
                "--workload",
                workload,
                "--backend",
                "mockllm",
                "--model",
                "mock-explicit",
                "--arm",
                arm,
                "--replicates",
                "4",
                "--vary",
                &format!("tool_order_stable={stable}"),
                "--hypothesis",
                "zz.toml",
            ],
        );
        assert_eq!(code, Some(0), "{json}");
        bundles.push(format!("runs/{}", json["run_id"].as_str().unwrap()));
    }
    let mut args = vec!["hyp", "verdict", "--hypothesis", "zz.toml"];
    args.extend(bundles.iter().rev().map(String::as_str));
    let (code, json) = acn_in(dir.path(), &args);
    assert_eq!(code, Some(0), "{json}");
    assert_eq!(json["ok"], true);
    let run_ids: Vec<&str> = json["run_ids"]
        .as_array()
        .unwrap()
        .iter()
        .map(|r| r.as_str().unwrap())
        .collect();
    assert_eq!(run_ids.len(), 3);
    let mut sorted = run_ids.clone();
    sorted.sort_unstable();
    assert_eq!(run_ids, sorted, "ascending, whatever the argument order");
    let listed: Vec<&str> = json["verdict"]["bundles"]
        .as_array()
        .unwrap()
        .iter()
        .map(|b| b["run_id"].as_str().unwrap())
        .collect();
    assert_eq!(run_ids, listed);
    let id = json["verdict_id"].as_str().unwrap();
    assert_eq!(json["verdict"]["verdict_id"], id);
    let rel = json["verdict_path"].as_str().unwrap();
    assert_eq!(rel, format!("runs/verdicts/{id}/verdict.json"));
    let path = dir.path().join(rel);
    let text = std::fs::read_to_string(&path).unwrap();
    assert!(text.ends_with('\n'));
    assert!(
        json["verdict"]["labels"]
            .as_array()
            .unwrap()
            .iter()
            .any(|l| l == "mock-gated")
    );
    // Never overwritten: the same set again is refused.
    let (code, json) = acn_in(dir.path(), &args);
    assert_eq!(code, Some(1));
    assert_eq!(json["ok"], false);
    assert!(
        json["error"]
            .as_str()
            .unwrap()
            .contains("never overwritten"),
        "{json}"
    );
    assert_eq!(std::fs::read_to_string(&path).unwrap(), text);
    // A directory that is not a bundle is refused.
    let (code, json) = acn_in(
        dir.path(),
        &["hyp", "verdict", "--hypothesis", "zz.toml", "."],
    );
    assert_eq!(code, Some(1), "{json}");
    assert!(
        json["error"].as_str().unwrap().contains("manifest.json"),
        "{json}"
    );
    // HYP-4: verdicts go under a `runs` directory and nowhere else.
    let mut elsewhere = args.clone();
    elsewhere.extend(["--runs-dir", "out"]);
    let (code, json) = acn_in(dir.path(), &elsewhere);
    assert_eq!(code, Some(1));
    assert!(json["error"].as_str().unwrap().contains("HYP-4"), "{json}");
    assert!(!dir.path().join("out").exists());
}

/// The long options a subcommand's help lists (help goes to stderr; stdout
/// keeps its one JSON object, CON-8).
fn options(args: &[&str]) -> Vec<String> {
    let out = Command::new(env!("CARGO_BIN_EXE_acn"))
        .args(args)
        .arg("--help")
        .output()
        .unwrap();
    let text = String::from_utf8(out.stderr).unwrap();
    let mut v: Vec<String> = text
        .split_whitespace()
        .filter(|w| w.starts_with("--"))
        .map(|w| w.trim_end_matches(',').to_owned())
        .collect();
    v.sort();
    v.dedup();
    v
}

/// Cites: HYP-25
#[test]
fn acn_hyp_offers_no_option_to_relax_a_hypothesis() {
    assert_eq!(
        options(&["hyp", "verdict"]),
        ["--help", "--hypothesis", "--runs-dir"]
    );
    assert_eq!(options(&["hyp", "lint"]), ["--help"]);
    let out = Command::new(env!("CARGO_BIN_EXE_acn"))
        .args(["hyp", "--help"])
        .output()
        .unwrap();
    let text = String::from_utf8(out.stderr).unwrap();
    let commands: Vec<&str> = text
        .lines()
        .skip_while(|l| !l.starts_with("Commands:"))
        .skip(1)
        .take_while(|l| l.starts_with("  "))
        .filter_map(|l| l.split_whitespace().next())
        .collect();
    assert_eq!(commands, ["lint", "verdict"]);
}
