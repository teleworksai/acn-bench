//! `acn hyp lint` (HYP-27) under the CLI contract (CON-8).

#![allow(clippy::unwrap_used, clippy::expect_used, clippy::panic)] // CON-19: tests are exempt

use std::process::Command;

fn acn(args: &[&str]) -> (Option<i32>, serde_json::Value) {
    let out = Command::new(env!("CARGO_BIN_EXE_acn"))
        .args(args)
        .output()
        .unwrap();
    let text = String::from_utf8(out.stdout).unwrap();
    (
        out.status.code(),
        serde_json::from_str(text.trim()).unwrap(),
    )
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
    assert!(json["witness"]["fires"].is_object());
    let (code, json) = acn(&["hyp", "lint", "/nonexistent/x.toml"]);
    assert_eq!(code, Some(1));
    assert_eq!(json["ok"], false);
    assert!(json["errors"][0].as_str().unwrap().contains("x.toml"));
}
