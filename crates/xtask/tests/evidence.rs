//! Evidence pages (SPEC 085 LOOP-30, SPEC 140 P16-20, P16-21), rendered by
//! `cargo xtask docs-inventory` from a real loop's committed report, twin and
//! verdicts.

#![allow(clippy::unwrap_used, clippy::expect_used, clippy::panic)] // CON-19: tests are exempt

mod common;

use std::fs;
use std::path::Path;

use common::{fixture, strings, xtask_at};

const LOOP: &str = "5c0466be088be5faa4c3cef71d46ca74bc2cf1f942e688762be7a610595db7f5";
const L1: &str = "da68f08906d1a49e4ed7e7629c0852c53da943e1babf1f4d8135089b079f9b19";
const L2: &str = "28e657208493f8401ea356499bb5d63eb13c899b23c73475b38869a5003b9112";

fn copy(from: &Path, to: &Path) {
    for e in walkdir::WalkDir::new(from) {
        let e = e.unwrap();
        let dest = to.join(e.path().strip_prefix(from).unwrap());
        if e.file_type().is_dir() {
            fs::create_dir_all(&dest).unwrap();
        } else {
            fs::copy(e.path(), &dest).unwrap();
        }
    }
}

/// The `ok` fixture with the loop's committed files.
fn root() -> tempfile::TempDir {
    let d = tempfile::tempdir().unwrap();
    copy(&fixture("ok"), d.path());
    copy(&fixture("evidence"), d.path());
    d
}

fn page(d: &Path) -> String {
    fs::read_to_string(d.join("docs/evidence/zz.md")).unwrap()
}

fn edit(d: &Path, rel: &str, from: &str, to: &str) {
    let p = d.join(rel);
    let text = fs::read_to_string(&p).unwrap();
    assert!(text.contains(from), "{rel} lacks {from}");
    fs::write(&p, text.replacen(from, to, 1)).unwrap();
}

fn report() -> String {
    format!("docs/runs/loop/{LOOP}/report.json")
}

fn twin() -> String {
    format!("docs/runs/loop/{LOOP}/twin/{L2}/twin.json")
}

fn fails(d: &Path, needle: &str) {
    let run = xtask_at(d, &["docs-inventory"]);
    assert!(!run.ok(), "{}", run.json);
    assert!(
        run.json.to_string().contains(needle),
        "{needle}: {}",
        run.json
    );
}

/// Cites: P16-20, P16-21
#[test]
fn a_committed_loop_renders_a_page_with_every_layer_and_its_chain() {
    let d = root();
    let run = xtask_at(d.path(), &["docs-inventory"]);
    assert!(run.ok(), "{}", run.json);
    let page = page(d.path());
    for needle in [
        "# Evidence: zz",
        &format!("## Loop `{LOOP}`"),
        // Each layer's verdict, with its labels, whether it is citable, and its reasons.
        &format!(
            "| L1 | fail | `{L1}` | exploratory, mock-gated, sim-only | no (exploratory, mock-gated, sim-only) | none |"
        ),
        &format!(
            "| L2 | fail | `{L2}` | exploratory, mock-gated | no (exploratory, mock-gated) | none |"
        ),
        "| L3 | none yet |",
        // The control effects with their intervals, numbers as committed.
        "| (none) | tool\\_order\\_stable=false | cached\\_token\\_ratio | 0.0 | [0.0, 0.0] | 4 / 4 |",
        "None recorded: the twin compared no quantity with a tolerance.",
        "The hypothesis has no `provider` parameter.",
        // The chain: the L1 bundles, and the L2 bundles beside what they twin.
        "87345904ab198d6b3062747c2dcbf8b35002cf8c47dd2378aaf4f379fc7be6fa",
        &format!("- L2 verdict: `{L2}`, the twin of L1 verdict `{L1}`; its reasons: none"),
        "| tool\\_order\\_stable=false | control | `d4fa117162983e9c2ac86fbe43be95f6880d291c2e070ed36c72f4a4c65589f9` | `9b9b1c92dc89556b2ac2ed2cb3f58dbe4c11219fddcbcaa4d98e3961bf3b0bfc` |",
    ] {
        assert!(page.contains(needle), "the page lacks `{needle}`\n{page}");
    }
    // Not an attribution quantity: no ATR-42 statement.
    assert!(!page.contains("ATR-42"));
    let check = xtask_at(d.path(), &["docs-inventory", "--check"]);
    assert!(check.ok(), "{}", check.json);
}

/// Cites: P16-21
#[test]
fn divergence_figures_are_written_as_committed() {
    let d = root();
    edit(
        d.path(),
        &twin(),
        "\"quantities\":{}",
        "\"quantities\":{\"cached_token_ratio\":{\"abs\": 1.10, \"within\": true}}",
    );
    assert!(xtask_at(d.path(), &["docs-inventory"]).ok());
    let page = page(d.path());
    // Compact, every digit as written: `1.10`, not `1.1`.
    assert!(
        page.contains(&format!(
            "| `{}` | (none) | tool\\_order\\_stable=false | cached\\_token\\_ratio | {{\"abs\":1.10,\"within\":true}} |",
            &L2[..12]
        )),
        "{page}"
    );
}

/// Cites: P16-21
#[test]
fn committed_text_cannot_write_the_page() {
    // Free text is escaped: a slice with Markdown in it stays one cell.
    let d = root();
    edit(
        d.path(),
        &report(),
        "\"slice\":\"\",\"treatment_replicates\"",
        "\"slice\":\"s`l|x\\n# h\",\"treatment_replicates\"",
    );
    assert!(xtask_at(d.path(), &["docs-inventory"]).ok());
    let page = page(d.path());
    assert!(page.contains("| s\\`l\\|x \\# h |"), "{page}");
    assert!(!page.contains("\n# h"));
    // Ids, enums and the seed are checked, not escaped.
    for (rel, from, to, needle) in [
        // The report's own fields, the ones followed by its `verdict_id`.
        (
            report(),
            "\"stop\":\"exhausted\",\"verdict\":\"fail\",\"verdict_id\"",
            "\"stop\":\"x\\n# stop\",\"verdict\":\"fail\",\"verdict_id\"",
            "not a plain word",
        ),
        (
            report(),
            "\"stop\":\"exhausted\",\"verdict\":\"fail\",\"verdict_id\"",
            "\"stop\":\"exhausted\",\"verdict\":\"fail\\n# v\",\"verdict_id\"",
            "not a verdict",
        ),
        (
            report(),
            "\"run_id\":\"87345904ab198d6b3062747c2dcbf8b35002cf8c47dd2378aaf4f379fc7be6fa\"",
            "\"run_id\":\"```\"",
            "not 64 lowercase hex",
        ),
    ] {
        let d = root();
        edit(d.path(), &rel, from, to);
        fails(d.path(), needle);
    }
}

/// Cites: P16-20
#[test]
fn committed_files_are_checked_against_what_they_name() {
    // A verdict edited by hand, its id kept: its id no longer recomputes.
    let d = root();
    let v = format!("docs/runs/verdicts/{L1}/verdict.json");
    edit(
        d.path(),
        &v,
        "c04bf7ac386a9978d2f674b7cec4a63c07e1c0a8241024bdf505267922a22937",
        &"0".repeat(64),
    );
    fails(d.path(), "does not recompute");
    // A twin of another L1 verdict.
    let d = root();
    edit(
        d.path(),
        &twin(),
        &format!("\"l1_verdict_id\":\"{L1}\""),
        &format!("\"l1_verdict_id\":\"{}\"", "1".repeat(64)),
    );
    fails(d.path(), "not its loop's final verdict");
    // A verdict named by nothing committed.
    let d = root();
    fs::remove_dir_all(d.path().join(format!("docs/runs/loop/{LOOP}/twin"))).unwrap();
    fails(d.path(), "named by no committed report or twin");
    // A file under another id than its own.
    let d = root();
    copy(
        &d.path().join(format!("docs/runs/verdicts/{L1}")),
        &d.path()
            .join(format!("docs/runs/verdicts/{}", "e".repeat(64))),
    );
    fails(d.path(), "is not its directory's name");
}

/// Cites: P16-21, ATR-42
#[test]
fn a_page_showing_an_attribution_quantity_says_its_network_is_emulated() {
    let d = root();
    let report = d.path().join(format!("docs/runs/loop/{LOOP}/report.json"));
    let text = fs::read_to_string(&report).unwrap();
    fs::write(
        &report,
        text.replace("\"cached_token_ratio\"", "\"network_attributable_share\""),
    )
    .unwrap();
    assert!(xtask_at(d.path(), &["docs-inventory"]).ok());
    let page = fs::read_to_string(d.path().join("docs/evidence/zz.md")).unwrap();
    assert!(
        page.contains("emulated network") && page.contains("ATR-42"),
        "{page}"
    );
}

/// Cites: P16-20, P16-21
#[test]
fn a_stale_page_or_a_missing_committed_verdict_fails() {
    let d = root();
    assert!(xtask_at(d.path(), &["docs-inventory"]).ok());
    // A page edited by hand is stale.
    let page = d.path().join("docs/evidence/zz.md");
    fs::write(&page, "edited\n").unwrap();
    let check = xtask_at(d.path(), &["docs-inventory", "--check"]);
    assert!(!check.ok(), "{}", check.json);
    assert!(
        strings(&check.json, "changed")
            .iter()
            .any(|f| f.ends_with("docs/evidence/zz.md"))
    );
    // A page for a hypothesis with no committed loop is stale too.
    fs::write(d.path().join("docs/evidence/old.md"), "x\n").unwrap();
    let check = xtask_at(d.path(), &["docs-inventory", "--check"]);
    assert!(
        strings(&check.json, "stale")
            .iter()
            .any(|f| f.ends_with("old.md")),
        "{}",
        check.json
    );
    // A twin's L2 verdict that is not committed fails, in both modes.
    fs::remove_dir_all(d.path().join(format!("docs/runs/verdicts/{L2}"))).unwrap();
    for args in [&["docs-inventory"][..], &["docs-inventory", "--check"][..]] {
        let run = xtask_at(d.path(), args);
        assert!(!run.ok(), "{}", run.json);
        assert!(
            run.json.to_string().contains("not committed"),
            "{}",
            run.json
        );
    }
}
