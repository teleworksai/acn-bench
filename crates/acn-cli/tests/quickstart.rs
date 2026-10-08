//! The README's quickstart (SPEC 140 P16-30): its lines, run as written, build
//! nothing new, run one `sim` bundle and regenerate it identical.

#![allow(clippy::unwrap_used, clippy::expect_used, clippy::panic)] // CON-19: tests are exempt

use std::path::Path;
use std::process::Command;

use serde_json::Value;

/// The kit's build command (P16-10).
const BUILD: &str = "cargo build -p acn-cli --locked";

fn root() -> &'static Path {
    Path::new(concat!(env!("CARGO_MANIFEST_DIR"), "/../.."))
}

/// The lines of the one `sh quickstart` block.
fn quickstart() -> Vec<String> {
    let readme = std::fs::read_to_string(root().join("README.md"))
        .unwrap()
        .replace("\r\n", "\n");
    let blocks: Vec<&str> = readme.split("```sh quickstart\n").skip(1).collect();
    assert_eq!(
        blocks.len(),
        1,
        "README.md carries one quickstart block (P16-30)"
    );
    blocks[0]
        .split("\n```")
        .next()
        .unwrap()
        .lines()
        .map(str::trim)
        .filter(|l| !l.is_empty())
        .map(str::to_owned)
        .collect()
}

/// Removes the lock file when the test ends, however it ends.
struct Lock(std::path::PathBuf);

impl Drop for Lock {
    fn drop(&mut self) {
        let _ = std::fs::remove_file(&self.0);
    }
}

/// Cites: P16-30, P16-10
#[test]
fn the_readme_quickstart_runs_as_written() {
    let lines = quickstart();
    assert_eq!(lines.first().map(String::as_str), Some(BUILD));
    // The test is built with the binary the first line builds, so it runs the
    // others with it rather than building again.
    // One quickstart at a time in this checkout: its run_id is fixed, and a
    // second run into the same directory is refused (CON-29).
    std::fs::create_dir_all(root().join("target")).unwrap();
    let lock = root().join("target/quickstart.lock");
    let _guard = loop {
        match std::fs::OpenOptions::new()
            .write(true)
            .create_new(true)
            .open(&lock)
        {
            Ok(_) => break Lock(lock.clone()),
            Err(e) if e.kind() == std::io::ErrorKind::AlreadyExists => {
                let (_tx, rx) = std::sync::mpsc::channel::<()>();
                let _ = rx.recv_timeout(std::time::Duration::from_millis(200));
            }
            Err(e) => panic!("{e}"),
        }
    };
    let scratch = root().join("target/quickstart");
    let _ = std::fs::remove_dir_all(&scratch);
    let mut run_id: Option<String> = None;
    let mut last = Value::Null;
    for line in &lines[1..] {
        let words: Vec<String> = line
            .split_whitespace()
            .map(|w| {
                if w == "$RUN_ID" {
                    run_id.clone().expect("a line before printed a run_id")
                } else {
                    w.to_owned()
                }
            })
            .collect();
        assert_eq!(
            words[0], "acn",
            "{line}: every other line starts with `acn`"
        );
        // No shell (CON-2): nothing a shell would read but `$RUN_ID`.
        let bare = line.replace("$RUN_ID", "");
        assert!(
            !bare.contains([
                '|', '>', '<', '*', '`', ';', '&', '$', '~', '\'', '"', '\\', '?', '[', '(', '{',
                '#'
            ]),
            "{line}: no shell (CON-2)"
        );
        let out = Command::new(env!("CARGO_BIN_EXE_acn"))
            .current_dir(root())
            .args(&words[1..])
            .output()
            .unwrap();
        let stdout = String::from_utf8(out.stdout).unwrap();
        assert!(out.status.success(), "{line}: {stdout}");
        last = serde_json::from_str(stdout.trim()).unwrap();
        if let Some(id) = last.get("run_id").and_then(Value::as_str) {
            run_id = Some(id.to_owned());
        }
    }
    assert_eq!(last["identical"], true, "{last}");
    let _ = std::fs::remove_dir_all(&scratch);
}

/// Cites: P16-10
#[test]
fn the_kits_byte_compared_files_keep_their_bytes() {
    let attrs = std::fs::read_to_string(root().join(".gitattributes")).unwrap();
    for line in [
        "kit/** -text",
        "lab/**/*.toml -text",
        "docs/runs/**/*.json -text",
    ] {
        assert!(attrs.lines().any(|l| l.trim() == line), "{line}");
    }
}
