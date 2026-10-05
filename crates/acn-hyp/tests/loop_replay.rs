//! LOOP-14: a loop report regenerates from the inputs it records, into a
//! fresh `runs/regen/<loop_id>/<n>/`, reusing no bundle; every bundle, both
//! report files and the final verdict compare byte for byte; a changed input or
//! another build is refused; and nothing is written outside that directory.

#![allow(clippy::unwrap_used, clippy::expect_used, clippy::panic)] // CON-19: tests are exempt

mod common;

use std::collections::BTreeMap;
use std::path::{Path, PathBuf};

use acn_hyp::loop_run::{self, Code};
use common::exec::{Exec, TWO, args, dir_with, run_loop};

/// Every file under `dir` but `runs/regen/`, with its bytes.
fn snapshot(dir: &Path) -> BTreeMap<PathBuf, Vec<u8>> {
    let mut out = BTreeMap::new();
    let mut stack = vec![dir.to_path_buf()];
    while let Some(d) = stack.pop() {
        for e in std::fs::read_dir(&d).unwrap() {
            let p = e.unwrap().path();
            if p.starts_with(dir.join("runs/regen")) {
                continue;
            }
            if p.is_dir() {
                stack.push(p);
            } else {
                out.insert(p.clone(), std::fs::read(&p).unwrap());
            }
        }
    }
    out
}

/// Cites: LOOP-14
#[test]
fn a_report_regenerates_byte_for_byte_into_a_fresh_directory() {
    let dir = dir_with(TWO);
    let d = dir.path();
    let c = run_loop(d, args(10), &mut Exec::new(d)).unwrap();
    let before = snapshot(d);
    let mut ex = Exec::new(d);
    let bin = ex.bin();
    let r = loop_run::regenerate(&c.report, bin, &mut ex).unwrap();
    assert!(r.identical(), "{:?}", r.differ);
    assert_eq!(r.loop_id, c.loop_id);
    // Paths come back canonical: the report path is resolved first.
    let regen = std::fs::canonicalize(d)
        .unwrap()
        .join("runs/regen")
        .join(c.loop_id.to_hex());
    assert_eq!(r.dir, regen.join("1"));
    assert_eq!(ex.requests.len(), 3, "no bundle is reused");
    assert!(ex.requests.iter().all(|q| q.runs_dir == regen.join("1")));
    for id in &c.run_ids {
        assert!(r.dir.join(id.to_hex()).join("manifest.json").exists());
    }
    let copy = r.dir.join("loop").join(c.loop_id.to_hex());
    assert_eq!(
        std::fs::read(copy.join("report.json")).unwrap(),
        std::fs::read(&c.report).unwrap()
    );
    assert!(
        !r.dir.join("verdicts").exists(),
        "verdicts are judged in memory"
    );
    assert_eq!(snapshot(d), before, "nothing written outside runs/regen/");
    // A second regeneration takes the next directory.
    let r2 = loop_run::regenerate(&c.report, bin, &mut Exec::new(d)).unwrap();
    assert!(r2.identical());
    assert_eq!(r2.dir, regen.join("2"));
}

/// Cites: LOOP-14
#[test]
fn a_difference_is_reported_and_a_changed_input_or_build_is_refused() {
    let dir = dir_with(TWO);
    let d = dir.path();
    let c = run_loop(d, args(10), &mut Exec::new(d)).unwrap();
    let bin = Exec::new(d).bin();
    // An edited rendering and an edited verdict differ from what regenerates.
    let md = c.report.with_file_name("report.md");
    std::fs::write(&md, "edited\n").unwrap();
    let vpath = d
        .join("runs/verdicts")
        .join(c.verdict_id.to_hex())
        .join("verdict.json");
    std::fs::write(&vpath, "{}\n").unwrap();
    // An original bundle that no longer verifies differs too.
    let first = c.run_ids[0].to_hex();
    std::fs::write(d.join("runs").join(&first).join("spans.parquet"), b"x").unwrap();
    let r = loop_run::regenerate(&c.report, bin, &mut Exec::new(d)).unwrap();
    assert!(!r.identical());
    assert_eq!(
        r.differ,
        ["report.md", "verdict.json", &format!("bundle {first}")]
    );

    // Another build cannot regenerate (CON-31): refused before anything runs.
    let mut other = Exec::with_build(d, "other");
    let other_bin = other.bin();
    let e = loop_run::regenerate(&c.report, other_bin, &mut other).unwrap_err();
    assert_eq!(e.code, Code::NotRegenerable, "{e}");
    assert_eq!(e.code.as_str(), "not_regenerable_with_this_build");
    assert!(other.requests.is_empty());

    // A changed workload or hypothesis file is refused.
    let w = d.join("w.toml");
    let text = std::fs::read_to_string(&w).unwrap();
    std::fs::write(&w, format!("{text}\n# edited\n")).unwrap();
    let mut ex = Exec::new(d);
    let e = loop_run::regenerate(&c.report, bin, &mut ex).unwrap_err();
    assert_eq!(e.code, Code::InputChanged, "{e}");
    std::fs::write(&w, text).unwrap();
    let h = d.join("zz.toml");
    std::fs::write(&h, format!("{TWO}# edited\n")).unwrap();
    let e = loop_run::regenerate(&c.report, bin, &mut ex).unwrap_err();
    assert_eq!(e.code, Code::InputChanged, "{e}");
    assert!(ex.requests.is_empty());
    // A file that is not a loop report.
    let e = loop_run::regenerate(&vpath, bin, &mut ex).unwrap_err();
    assert_eq!(e.code, Code::Report, "{e}");
}

/// Cites: LOOP-14
#[test]
fn a_regeneration_that_makes_other_bytes_says_which() {
    let dir = dir_with(TWO);
    let d = dir.path();
    let c = run_loop(d, args(10), &mut Exec::new(d)).unwrap();
    // The same inputs on a mock that caches differently: every bundle, the
    // report and the verdict come out with other bytes.
    let mut ex = Exec::new(d);
    ex.profiles = acn_mockllm::profile::Profiles::parse(
        &acn_mockllm::profile::PROFILES_TOML
            .replace("min_cacheable_tokens = 1024", "min_cacheable_tokens = 64"),
    )
    .unwrap();
    let bin = ex.bin();
    let r = loop_run::regenerate(&c.report, bin, &mut ex).unwrap();
    assert!(!r.identical());
    assert!(
        r.differ.contains(&"report.json".to_owned()),
        "{:?}",
        r.differ
    );
    for id in &c.run_ids {
        assert!(
            r.differ.contains(&format!("bundle {}", id.to_hex())),
            "{id}: {:?}",
            r.differ
        );
    }
}

/// Cites: LOOP-14
#[test]
fn a_workload_gone_since_the_report_is_an_input_change() {
    let dir = dir_with(TWO);
    let d = dir.path();
    let c = run_loop(d, args(10), &mut Exec::new(d)).unwrap();
    std::fs::remove_file(d.join("w.toml")).unwrap();
    let mut ex = Exec::new(d);
    let bin = ex.bin();
    let e = loop_run::regenerate(&c.report, bin, &mut ex).unwrap_err();
    assert_eq!(e.code, Code::InputChanged, "{e}");
    assert!(e.message.contains("can no longer be read"), "{e}");
    assert!(ex.requests.is_empty());
}
