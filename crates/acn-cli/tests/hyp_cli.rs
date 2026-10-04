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
