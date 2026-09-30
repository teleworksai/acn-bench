//! The `acn` binary's CLI contract (CON-8).

#![allow(clippy::unwrap_used, clippy::expect_used, clippy::panic)] // CON-19: tests are exempt

use std::process::Command;

fn acn(args: &[&str]) -> (Option<i32>, serde_json::Value) {
    let out = Command::new(env!("CARGO_BIN_EXE_acn"))
        .args(args)
        .output()
        .expect("spawn acn");
    let stdout = String::from_utf8(out.stdout).expect("utf8");
    let json: serde_json::Value = serde_json::from_str(stdout.trim())
        .unwrap_or_else(|e| panic!("stdout is not one JSON object: {e}\n{stdout}"));
    assert!(json.is_object());
    (out.status.code(), json)
}

/// Cites: CON-8
#[test]
fn version_prints_one_json_object_and_exits_zero() {
    let (code, json) = acn(&["version"]);
    assert_eq!(json["ok"], true);
    assert_eq!(code, Some(0));
    assert!(json["version"].is_string());
}

/// Cites: CON-8
#[test]
fn unknown_subcommand_is_a_json_error_with_exit_one() {
    let (code, json) = acn(&["definitely-not-a-subcommand"]);
    assert_eq!(json["ok"], false);
    assert_eq!(code, Some(1));
    assert!(json["error"].is_string());
}

/// Cites: CON-8
#[test]
fn help_version_and_no_arguments_keep_stdout_to_one_json_object() {
    for args in [
        &["--help"][..],
        &["--version"][..],
        &["version", "--help"][..],
    ] {
        let (code, json) = acn(args);
        assert_eq!(json["ok"], true, "{args:?}");
        assert_eq!(code, Some(0), "{args:?}");
    }
    let (code, json) = acn(&[]);
    assert_eq!(json["ok"], false);
    assert_eq!(code, Some(1));
}
