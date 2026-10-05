//! LOOP-3, HYP-4: an L1 result changes nothing it may not change. A loop over a
//! frozen hypothesis, with `hypotheses/` writable, writes only under the
//! workspace's own `runs/`; a loop pointed at any other directory is refused;
//! and a `fail` on a frozen file is written like any verdict, never an edit.

#![allow(clippy::unwrap_used, clippy::expect_used, clippy::panic)] // CON-19: tests are exempt

mod common;

use std::collections::BTreeMap;
use std::path::Path;

use acn_hyp::loop_run::{self, Code};
use acn_trace::identity::Digest;
use common::exec::{Exec, TWO, args, args_in, smoke};

/// Every entry under `dir` but `runs/`: a file with its bytes, a directory as
/// `None`.
fn snapshot(dir: &Path) -> BTreeMap<String, Option<Vec<u8>>> {
    let mut out = BTreeMap::new();
    let mut stack = vec![dir.to_path_buf()];
    while let Some(d) = stack.pop() {
        for e in std::fs::read_dir(&d).unwrap() {
            let p = e.unwrap().path();
            let rel = p.strip_prefix(dir).unwrap().to_string_lossy().into_owned();
            if rel == "runs" || rel.starts_with("runs/") {
                continue;
            }
            if p.is_dir() {
                out.insert(rel, None);
                stack.push(p);
            } else {
                out.insert(rel, Some(std::fs::read(&p).unwrap()));
            }
        }
    }
    out
}

/// TWO frozen under a workspace root: a spec, 20 replicates, and a falsifier
/// that fires on the mock (no cell's effect is exactly zero there), so the
/// loop ends on a `fail`.
fn frozen_root() -> tempfile::TempDir {
    let text = TWO
        .replace(
            "title = \"tool order and the cache\"",
            "title = \"tool order and the cache\"\nspec = \"specs/100-x.md\"",
        )
        .replace("replicates = 4", "replicates = 20")
        .replace("replicates < 4", "replicates < 20");
    common::root(&[("hypotheses/zz.toml", &text), ("w.toml", &smoke())])
}

/// Cites: LOOP-3, HYP-4, HYP-25
#[test]
fn a_loop_on_a_frozen_hypothesis_writes_only_under_runs() {
    let root = frozen_root();
    let r = root.path();
    let h = acn_hyp::load_in(&r.join("hypotheses/zz.toml"), r).unwrap();
    assert_eq!(h.status(), acn_hyp::Status::Frozen);
    let before = snapshot(r);
    // The workspace's own engine_hash, as its preflight requires (CON-28).
    let engine = acn_trace::env::compute(r).unwrap().engine_hash;
    let mut ex = Exec::new(r);
    ex.engine = Digest::from_hex(&engine).unwrap();
    let bin = ex.bin();
    let c = loop_run::run(&h, &args_in(r, args(3)), &r.join("runs"), bin, &mut ex).unwrap();
    assert_eq!(snapshot(r), before, "nothing outside runs/ changed");
    // Whatever the verdict, it is a written result with the file's status.
    let v: serde_json::Value = serde_json::from_str(
        &std::fs::read_to_string(
            r.join("runs/verdicts")
                .join(c.verdict_id.to_hex())
                .join("verdict.json"),
        )
        .unwrap(),
    )
    .unwrap();
    assert_eq!(v["hypothesis"]["status"], "frozen");
    assert_eq!(v["verdict"], c.verdict.as_str());

    // A loop aimed elsewhere is refused before anything runs (HYP-4).
    for bad in ["hypotheses/runs", "runs/verdicts", "specs/runs"] {
        std::fs::create_dir_all(r.join(bad).parent().unwrap()).unwrap();
        let mut ex = Exec::new(r);
        let e = loop_run::run(&h, &args_in(r, args(30)), &r.join(bad), bin, &mut ex).unwrap_err();
        assert_eq!(e.code, Code::Path, "{bad}: {e}");
        assert!(ex.requests.is_empty());
        assert!(!r.join(bad).join("loop").exists());
    }
}
