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

/// A twin's `quantities` object holding one `cached_token_ratio` entry, in the
/// shape `acn loop twin` writes (LOOP-12), figures as given.
fn quantity(tolerance: &str, treatment: &str, control: &str, effect: &str, within: bool) -> String {
    format!(
        "\"quantities\":{{\"cached_token_ratio\":{{\"tolerance\":{tolerance},\"treatment\": {treatment},\"control\": {control},\"effect\": {effect},\"within\": {within}}}}}"
    )
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
        &format!("## Loop `{LOOP}` (candidate `target/evfix/zz.toml`)"),
        "| File | `target/evfix/zz.toml` |",
        "A loop marked candidate ran the candidate file its heading names",
        // The slices of each layer's verdict.
        &format!("| L1 | `{}` | (none) | fail | none |", &L1[..12]),
        &format!("| L2 | `{}` | (none) | fail | none |", &L2[..12]),
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
        &quantity("{\"abs\": 1.10}", "0.50", "0.25", "null", true),
    );
    assert!(xtask_at(d.path(), &["docs-inventory"]).ok());
    let page = page(d.path());
    // Compact, every digit as written: `1.10`, not `1.1`.
    assert!(
        page.contains(&format!(
            "| `{}` | (none) | tool\\_order\\_stable=false | cached\\_token\\_ratio | abs 1.10 | 0.50 | 0.25 | null | yes |",
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

/// Cites: P16-20, LOOP-12
#[test]
fn a_twins_within_flag_is_recomputed_from_its_figures() {
    let abs = "{\"abs\":0.01}";
    let rel = "{\"relative\":0.05}";
    // (tolerance, treatment, control, effect, the flag the figures give)
    let cases = [
        // Every figure at or under the tolerance, the bound included.
        (abs, "0.0", "0.01", "0.002", true),
        (rel, "0.05", "0.0", "0.049", true),
        (abs, "1e-2", "-0.5", "0", true),
        // One figure over, by a lot or by the last place of a double.
        (abs, "0.011", "0.0", "0.0", false),
        (abs, "0.0", "0.02", "null", false),
        (rel, "0.0", "0.0", "0.06", false),
        (
            "{\"abs\":0.03}",
            "0.030000000000000002",
            "0.0",
            "0.0",
            false,
        ),
        // An arm whose divergence is undefined (HYP-22).
        (abs, "null", "0.0", "0.0", false),
        (abs, "0.0", "null", "null", false),
    ];
    for (tol, t, c, e, want) in cases {
        // As the figures give it: accepted.
        let d = root();
        edit(
            d.path(),
            &twin(),
            "\"quantities\":{}",
            &quantity(tol, t, c, e, want),
        );
        let run = xtask_at(d.path(), &["docs-inventory"]);
        assert!(run.ok(), "{tol} {t} {c} {e} {want}: {}", run.json);
        // Flipped, either way: refused, naming the cell and the quantity.
        let d = root();
        edit(
            d.path(),
            &twin(),
            "\"quantities\":{}",
            &quantity(tol, t, c, e, !want),
        );
        let run = xtask_at(d.path(), &["docs-inventory"]);
        let text = run.json.to_string();
        assert!(!run.ok(), "{tol} {t} {c} {e} {}: {text}", !want);
        for needle in [
            "`within` is written",
            "cached_token_ratio",
            "tool_order_stable=false",
        ] {
            assert!(text.contains(needle), "{needle}: {text}");
        }
    }
}

/// Cites: P16-20, LOOP-12
#[test]
fn a_null_effect_is_undefined_where_the_twin_reads_that_quantitys_effect() {
    let fine =
        |effect: &str, within: bool| quantity("{\"abs\":0.01}", "0.0", "0.0", effect, within);
    // The second cell's entry, after the first cell holds `first`.
    let two = |first: &str, second: &str| {
        let d = root();
        edit(d.path(), &twin(), "\"quantities\":{}", first);
        edit(d.path(), &twin(), "\"quantities\":{}", second);
        d
    };
    // The effect is defined in one cell, so the twin reads this quantity's
    // effect: `null` in the other cell is undefined, and not within.
    assert!(
        xtask_at(
            two(&fine("0.001", true), &fine("null", false)).path(),
            &["docs-inventory"]
        )
        .ok()
    );
    fails(
        two(&fine("0.001", true), &fine("null", true)).path(),
        "`within` is written true",
    );
    // Another quantity's defined effect says nothing about this one's.
    let other = fine("0.001", true).replace("cached_token_ratio", "cost_per_success");
    assert!(xtask_at(two(&other, &fine("null", true)).path(), &["docs-inventory"]).ok());
    // Where no cell shows whether the effect is read, the file is taken at
    // its word either way (ADR-47): `false` can only fail a gate.
    for within in [true, false] {
        let d = root();
        edit(
            d.path(),
            &twin(),
            "\"quantities\":{}",
            &fine("null", within),
        );
        assert!(xtask_at(d.path(), &["docs-inventory"]).ok(), "{within}");
    }
}

/// Cites: P16-20, LOOP-12
#[test]
fn a_twin_quantity_of_another_shape_is_refused() {
    let entry = |body: &str| format!("\"quantities\":{{\"q\":{{{body}}}}}");
    let ok = "\"tolerance\":{\"abs\":0.01},\"treatment\":0.0,\"control\":0.0,\"effect\":null,\"within\":true";
    // The whole entry is accepted; each change to it is refused for its shape.
    let d = root();
    edit(d.path(), &twin(), "\"quantities\":{}", &entry(ok));
    assert!(xtask_at(d.path(), &["docs-inventory"]).ok());
    for (from, to) in [
        // A key missing: each figure, the flag, the tolerance.
        ("\"treatment\":0.0,", ""),
        ("\"control\":0.0,", ""),
        ("\"effect\":null,", ""),
        (",\"within\":true", ""),
        ("\"tolerance\":{\"abs\":0.01},", ""),
        // A key the writer does not write.
        ("\"within\":true", "\"within\":true,\"note\":1"),
        // A tolerance of no kind, of both kinds, of another kind.
        ("{\"abs\":0.01}", "{}"),
        ("{\"abs\":0.01}", "{\"abs\":0.01,\"relative\":0.01}"),
        ("{\"abs\":0.01}", "{\"about\":0.01}"),
        ("{\"abs\":0.01}", "{\"abs\":\"0.01\"}"),
        // A figure that is not a number.
        ("\"treatment\":0.0", "\"treatment\":\"0.0\""),
        ("\"control\":0.0", "\"control\":true"),
        ("\"effect\":null", "\"effect\":[0.0]"),
    ] {
        assert!(ok.contains(from), "{from}");
        let d = root();
        edit(
            d.path(),
            &twin(),
            "\"quantities\":{}",
            &entry(&ok.replacen(from, to, 1)),
        );
        fails(d.path(), "not a twin's figures");
    }
    // The first shape this page was once given.
    let d = root();
    edit(
        d.path(),
        &twin(),
        "\"quantities\":{}",
        "\"quantities\":{\"q\":{\"abs\": 1.10, \"within\": true}}",
    );
    fails(d.path(), "not a twin's figures");
}

/// The fixture's candidate, written into the tree with other bytes than its
/// loop ran.
fn edited_candidate(d: &Path) {
    let path = d.join("target/evfix/zz.toml");
    fs::create_dir_all(path.parent().unwrap()).unwrap();
    fs::write(&path, "# edited after the loop\n").unwrap();
}

/// Cites: P16-20, HYP-5
#[test]
fn a_candidate_edited_after_its_loop_fails() {
    // The fixture's candidate is not in the tree: nothing to compare.
    let d = root();
    assert!(xtask_at(d.path(), &["docs-inventory"]).ok());
    // In the tree with other bytes than the loop ran: refused.
    edited_candidate(d.path());
    fails(d.path(), "target/evfix/zz.toml no longer has the bytes");
    fails(d.path(), "HYP-5");
    // Not a regular file: refused, a link's target never read.
    let d = root();
    fs::create_dir_all(d.path().join("target/evfix/zz.toml")).unwrap();
    fails(d.path(), "not a regular file");
    #[cfg(unix)]
    {
        let d = root();
        fs::create_dir_all(d.path().join("target/evfix")).unwrap();
        fs::write(d.path().join("elsewhere.toml"), "x").unwrap();
        std::os::unix::fs::symlink(
            d.path().join("elsewhere.toml"),
            d.path().join("target/evfix/zz.toml"),
        )
        .unwrap();
        fails(d.path(), "not a regular file");
    }
}

/// Cites: P16-20, HYP-3, HYP-5
#[test]
fn a_report_cannot_talk_its_way_out_of_the_candidate_check() {
    let path = "\"path\":\"target/evfix/zz.toml\"";
    let status = "\"status\":\"candidate\"";
    // Each edit is made to the report and to the verdicts that carry the
    // same hypothesis object where they must agree; the candidate is edited.
    for (from, to, needle) in [
        // Called frozen, outside hypotheses/.
        (status, "\"status\":\"frozen\"", "is not under hypotheses/"),
        // A status that is neither.
        (
            status,
            "\"status\":\"candidat\"",
            "neither `candidate` nor `frozen`",
        ),
        // No path, an empty one, one that leaves the tree, an absolute one.
        (path, "\"path\":\"\"", "not a path inside the repository"),
        (
            path,
            "\"path\":\"../zz.toml\"",
            "not a path inside the repository",
        ),
        (
            path,
            "\"path\":\"/etc/hosts\"",
            "not a path inside the repository",
        ),
        (
            path,
            "\"path\":\"./target/evfix/zz.toml\"",
            "not a path inside the repository",
        ),
    ] {
        let d = root();
        edited_candidate(d.path());
        edit(d.path(), &report(), from, to);
        fails(d.path(), needle);
    }
    let d = root();
    edited_candidate(d.path());
    edit(d.path(), &report(), &format!(",{path}"), "");
    fails(d.path(), "not a path inside the repository");
    // A file under hypotheses/ is env-hash's, whatever its loop called it: an
    // amendment after the loop is not this check's to refuse (CON-7).
    for word in ["candidate", "frozen"] {
        let d = root();
        fs::create_dir_all(d.path().join("hypotheses")).unwrap();
        fs::write(d.path().join("hypotheses/zz.toml"), "# amended\n").unwrap();
        edit(d.path(), &report(), path, "\"path\":\"hypotheses/zz.toml\"");
        edit(
            d.path(),
            &report(),
            status,
            &format!("\"status\":\"{word}\""),
        );
        let run = xtask_at(d.path(), &["docs-inventory"]);
        assert!(run.ok(), "{word}: {}", run.json);
    }
}

/// Cites: P16-20, HYP-5
#[test]
fn this_repositorys_committed_candidates_are_in_the_tree_as_their_loops_ran_them() {
    // The fixtures cannot hold a candidate with its loop's bytes, and a
    // candidate moved away is not compared, so the real tree is checked here:
    // every committed loop of a file outside hypotheses/ still has that file,
    // with the bytes it ran. At least one such loop is committed (GATE-23's).
    let root = Path::new(env!("CARGO_MANIFEST_DIR")).join("../..");
    let mut seen = 0;
    for e in fs::read_dir(root.join("docs/runs/loop")).unwrap() {
        let report: serde_json::Value = serde_json::from_str(
            &fs::read_to_string(e.unwrap().path().join("report.json")).unwrap(),
        )
        .unwrap();
        let h = &report["hypothesis"];
        let rel = h["path"].as_str().unwrap();
        if rel.starts_with("hypotheses/") {
            continue;
        }
        let file = root.join(rel);
        assert!(
            file.is_file(),
            "{rel}: a committed loop's candidate left the tree"
        );
        assert_eq!(
            blake3::hash(&fs::read(&file).unwrap()).to_hex().as_str(),
            h["hash"].as_str().unwrap(),
            "{rel}"
        );
        seen += 1;
    }
    assert!(seen > 0, "no committed loop of a candidate");
}

/// The position of `needle` in `page`, which must hold it.
fn at(page: &str, needle: &str) -> usize {
    page.find(needle)
        .unwrap_or_else(|| panic!("the page lacks `{needle}`\n{page}"))
}

/// Cites: P16-21, HYP-24
#[test]
fn each_layer_has_its_own_provider_table_and_slice_reasons() {
    let d = root();
    let (l1, l2) = (
        format!("docs/runs/verdicts/{L1}/verdict.json"),
        format!("docs/runs/verdicts/{L2}/verdict.json"),
    );
    // L1 passes its provider; at L2 the twin rule makes it inconclusive, as
    // on GATE-23's loop of record (ADR-46).
    edit(
        d.path(),
        &l1,
        "\"providers\":null",
        "\"providers\":{\"aa\":\"pass\"}",
    );
    edit(
        d.path(),
        &l2,
        "\"providers\":null",
        "\"providers\":{\"aa\":\"inconclusive\"}",
    );
    // The verdict's own reasons come first in the file, then its slice's.
    edit(
        d.path(),
        &l2,
        "\"reasons\":[],\"replicates\"",
        "\"reasons\":[{\"reason\":\"slice_inconclusive\",\"refers\":[\"\"]}],\"replicates\"",
    );
    edit(
        d.path(),
        &l2,
        "\"reasons\":[],\"replicates\"",
        "\"reasons\":[{\"reason\":\"twin_failed\",\"refers\":[\"a\",\"b\",\"c\"]},{\"reason\":\"undefined_value\",\"refers\":[\"x\"]}],\"replicates\"",
    );
    let run = xtask_at(d.path(), &["docs-inventory"]);
    assert!(run.ok(), "{}", run.json);
    let page = page(d.path());
    // One table per layer, each under its layer and verdict, in layer order.
    let t1 = at(&page, &format!("L1, verdict `{}`:", &L1[..12]));
    let t2 = at(&page, &format!("L2, verdict `{}`:", &L2[..12]));
    let pass = at(&page, "| aa | pass |");
    let inconclusive = at(&page, "| aa | inconclusive |");
    assert!(t1 < pass && pass < t2 && t2 < inconclusive, "{page}");
    assert!(!page.contains("has no `provider` parameter"));
    // The slice's own reasons, each with how many things it refers to.
    at(
        &page,
        &format!(
            "| L2 | `{}` | (none) | fail | twin\\_failed (3), undefined\\_value (1) |",
            &L2[..12]
        ),
    );
    at(
        &page,
        &format!("| L1 | `{}` | (none) | fail | none |", &L1[..12]),
    );
    // The verdict's row keeps the verdict's reasons, not its slices'.
    at(
        &page,
        &format!(
            "| L2 | fail | `{L2}` | exploratory, mock-gated | no (exploratory, mock-gated) | slice\\_inconclusive |"
        ),
    );
}

/// Cites: P16-21, LOOP-12
#[test]
fn the_divergence_table_says_how_many_quantities_are_within() {
    let d = root();
    edit(
        d.path(),
        &twin(),
        "\"quantities\":{}",
        &quantity("{\"abs\":0.01}", "0.0", "0.0", "0.001", true),
    );
    edit(
        d.path(),
        &twin(),
        "\"quantities\":{}",
        &quantity("{\"relative\":0.05}", "0.2", "0.0", "0.0", false),
    );
    assert!(xtask_at(d.path(), &["docs-inventory"]).ok());
    let page = page(d.path());
    at(
        &page,
        "1 of 2 twinned quantities are within their tolerance (CON-25). GATE-23 asks that none be outside.",
    );
    at(
        &page,
        "| cached\\_token\\_ratio | abs 0.01 | 0.0 | 0.0 | 0.001 | yes |",
    );
    at(
        &page,
        "| cached\\_token\\_ratio | relative 0.05 | 0.2 | 0.0 | 0.0 | no |",
    );
}

/// Cites: P16-21
#[test]
fn the_sections_a_gate_reads_come_before_the_control_effects() {
    let d = root();
    assert!(xtask_at(d.path(), &["docs-inventory"]).ok());
    let page = page(d.path());
    let order = [
        "### Verdicts",
        "### Divergence, sim against live (LOOP-12)",
        "### Providers (HYP-24)",
        "### Slices",
        "### Chain",
        "### Control effects (CON-18)",
    ]
    .map(|h| at(&page, h));
    assert!(order.is_sorted(), "{order:?}\n{page}");
}

/// Cites: P16-21, HYP-3
#[test]
fn a_frozen_hypothesiss_loop_is_not_marked_and_says_so_only_for_candidates() {
    // The same loop, of a file under hypotheses/: no candidate heading, and
    // no statement about candidates on the page.
    let d = root();
    edit(
        d.path(),
        &report(),
        "\"path\":\"target/evfix/zz.toml\"",
        "\"path\":\"hypotheses/zz.toml\"",
    );
    assert!(xtask_at(d.path(), &["docs-inventory"]).ok());
    let page = page(d.path());
    at(&page, &format!("## Loop `{LOOP}`\n"));
    at(&page, "| File | `hypotheses/zz.toml` |");
    assert!(!page.contains("candidate `"), "{page}");
    assert!(!page.contains("A loop marked candidate"), "{page}");
}
