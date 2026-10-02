//! Tests for `cargo xtask docs-inventory` (gate step in CON-9).

#![allow(clippy::unwrap_used, clippy::expect_used, clippy::panic)] // CON-19: tests are exempt

mod common;

use std::fs;

use common::{fixture, repo_root, strings, xtask_at};

fn copy_dir(from: &std::path::Path, to: &std::path::Path) {
    for entry in walkdir::WalkDir::new(from) {
        let entry = entry.expect("walk");
        let rel = entry.path().strip_prefix(from).expect("prefix");
        let dest = to.join(rel);
        if entry.file_type().is_dir() {
            fs::create_dir_all(&dest).expect("mkdir");
        } else {
            fs::copy(entry.path(), &dest).expect("copy");
        }
    }
}

/// Cites: CON-9
#[test]
fn check_fails_until_generated_docs_are_written_then_passes() {
    let dir = tempfile::tempdir().expect("tempdir");
    copy_dir(&fixture("ok"), dir.path());

    let before = xtask_at(dir.path(), &["docs-inventory", "--check"]);
    assert!(!before.ok(), "{}", before.json);
    assert!(!strings(&before.json, "changed").is_empty());

    let written = xtask_at(dir.path(), &["docs-inventory"]);
    assert!(written.ok(), "{}", written.json);
    let files = strings(&written.json, "written");
    assert!(
        files
            .iter()
            .any(|f| f.ends_with("docs/generated/requirements.md")),
        "{files:?}"
    );
    let req = fs::read_to_string(dir.path().join("docs/generated/requirements.md")).expect("read");
    assert!(req.contains("FIX-1"), "{req}");
    assert!(req.contains("fixture_is_cited"), "{req}");

    let after = xtask_at(dir.path(), &["docs-inventory", "--check"]);
    assert!(after.ok(), "{}", after.json);

    fs::write(dir.path().join("docs/generated/requirements.md"), "stale\n").expect("write");
    let stale = xtask_at(dir.path(), &["docs-inventory", "--check"]);
    assert!(!stale.ok(), "{}", stale.json);
}

/// Cites: CON-9
#[test]
fn generation_is_deterministic() {
    let dir = tempfile::tempdir().expect("tempdir");
    copy_dir(&fixture("ok"), dir.path());
    assert!(xtask_at(dir.path(), &["docs-inventory"]).ok());
    let first =
        fs::read_to_string(dir.path().join("docs/generated/requirements.md")).expect("read");
    assert!(xtask_at(dir.path(), &["docs-inventory"]).ok());
    let second =
        fs::read_to_string(dir.path().join("docs/generated/requirements.md")).expect("read");
    assert_eq!(first, second);
}

/// Cites: CON-9
#[test]
fn self_host_generated_docs_are_current() {
    let run = xtask_at(&repo_root(), &["docs-inventory", "--check"]);
    assert!(run.ok(), "{}", run.json);
}

/// Cites: CON-9
#[test]
fn stale_files_fail_the_check_and_a_write_removes_them() {
    let dir = tempfile::tempdir().expect("tempdir");
    copy_dir(&fixture("ok"), dir.path());
    assert!(xtask_at(dir.path(), &["docs-inventory"]).ok());
    for stale in ["old.md", ".hidden.md"] {
        fs::write(dir.path().join("docs/generated").join(stale), "x\n").expect("write");
    }
    let check = xtask_at(dir.path(), &["docs-inventory", "--check"]);
    assert!(!check.ok(), "{}", check.json);
    assert_eq!(
        strings(&check.json, "stale"),
        vec!["docs/generated/.hidden.md", "docs/generated/old.md"]
    );
    let write = xtask_at(dir.path(), &["docs-inventory"]);
    assert!(write.ok(), "{}", write.json);
    assert_eq!(strings(&write.json, "removed").len(), 2, "{}", write.json);
    assert!(
        xtask_at(dir.path(), &["docs-inventory", "--check"]).ok(),
        "a write must converge"
    );
}

/// Cites: CON-9
#[test]
fn decision_records_are_ordered_numerically_with_a_name_tie_break() {
    let dir = tempfile::tempdir().expect("tempdir");
    copy_dir(&fixture("ok"), dir.path());
    let adr = dir.path().join("docs/decisions");
    fs::create_dir_all(&adr).expect("mkdir");
    for (name, title) in [
        ("ADR-10.md", "Ten"),
        ("ADR-9.md", "Nine | piped"),
        ("ADR-zeta.md", "Zeta"),
        ("ADR-alpha.md", "Alpha"),
    ] {
        fs::write(
            adr.join(name),
            format!(
                "# {} — {title}\n\n**Status:** accepted. **IDs affected:** FIX-1.\n",
                name.trim_end_matches(".md")
            ),
        )
        .expect("write");
    }
    assert!(xtask_at(dir.path(), &["docs-inventory"]).ok());
    let md = fs::read_to_string(dir.path().join("docs/generated/decisions.md")).expect("read");
    let order: Vec<usize> = ["ADR-9]", "ADR-10]", "ADR-alpha]", "ADR-zeta]"]
        .iter()
        .map(|n| md.find(n).expect(n))
        .collect();
    assert!(order.windows(2).all(|w| w[0] < w[1]), "{md}");
    assert!(md.contains("Nine \\| piped"), "pipes are escaped: {md}");
    assert!(md.contains("| accepted | FIX-1 |"), "{md}");
}

/// Cites: CON-9
#[cfg(unix)]
#[test]
fn a_symlinked_output_is_refused_so_nothing_outside_the_directory_is_overwritten() {
    let dir = tempfile::tempdir().expect("tempdir");
    copy_dir(&fixture("ok"), dir.path());
    fs::create_dir_all(dir.path().join("docs/generated")).expect("mkdir");
    fs::write(dir.path().join("victim.txt"), "precious\n").expect("write");
    std::os::unix::fs::symlink(
        dir.path().join("victim.txt"),
        dir.path().join("docs/generated/requirements.md"),
    )
    .expect("symlink");
    let run = xtask_at(dir.path(), &["docs-inventory"]);
    assert!(!run.ok(), "{}", run.json);
    assert_eq!(
        fs::read_to_string(dir.path().join("victim.txt")).expect("read"),
        "precious\n"
    );
}

// ---- TRC-20: the attribute page, and the rule that nothing emits an unlisted `acn.*` name.

/// The `ok` fixture plus the repository's real schema files and one producer source file.
fn fixture_with_schema(producer_source: &str) -> tempfile::TempDir {
    let dir = tempfile::tempdir().expect("tempdir");
    copy_dir(&fixture("ok"), dir.path());
    let schema = dir.path().join("crates/acn-trace/src/schema");
    fs::create_dir_all(&schema).expect("mkdir");
    for f in ["acn_attributes.toml", "SEMCONV_VERSION"] {
        fs::copy(
            repo_root().join("crates/acn-trace/src/schema").join(f),
            schema.join(f),
        )
        .expect("copy schema");
    }
    let src = dir.path().join("crates/fix/src");
    fs::create_dir_all(&src).expect("mkdir");
    fs::write(src.join("lib.rs"), producer_source).expect("write");
    dir
}

/// Cites: TRC-20
#[test]
fn the_attribute_page_is_generated_from_the_inventory() {
    let dir = fixture_with_schema("pub fn nothing() {}\n");
    let run = xtask_at(dir.path(), &["docs-inventory"]);
    assert!(run.ok(), "{}", run.json);
    let page =
        fs::read_to_string(dir.path().join("docs/generated/acn-attributes.md")).expect("page");
    for needle in [
        "acn.call.new_input_tokens",
        "| `acn.link.applied_delay_ms` | float | ms |",
        "opt.stall_threshold_ms",
        "usage.cache_read_input_tokens",
        "1.41.0",
        "the provider returned usage",
    ] {
        assert!(page.contains(needle), "the page lacks `{needle}`");
    }
    // Without a schema (the small fixtures) there is no page, and that is not an error.
    let bare = tempfile::tempdir().expect("tempdir");
    copy_dir(&fixture("ok"), bare.path());
    assert!(xtask_at(bare.path(), &["docs-inventory"]).ok());
    assert!(
        !bare
            .path()
            .join("docs/generated/acn-attributes.md")
            .exists()
    );
}

/// Cites: TRC-20
#[test]
fn an_unlisted_acn_name_in_a_producer_fails_even_inside_a_macro() {
    let listed = "pub fn f() { let _ = (\"acn.session\", \"acn.call.index\"); emit!(\"acn.turn.outcome\" = \"success\"); }\n";
    let dir = fixture_with_schema(listed);
    let run = xtask_at(dir.path(), &["docs-inventory"]);
    assert!(run.ok(), "{}", run.json);

    let unlisted = "pub fn f() { emit!(\"acn.session\", \"acn.bogus.thing\" = 1); }\n// \"acn.in.a.comment\" is not emitted\n";
    let dir = fixture_with_schema(unlisted);
    for args in [&["docs-inventory"][..], &["docs-inventory", "--check"][..]] {
        let run = xtask_at(dir.path(), args);
        assert!(!run.ok(), "{}", run.json);
        let found = run.json["unlisted_attributes"]
            .as_array()
            .expect("unlisted_attributes");
        assert_eq!(found.len(), 1, "{}", run.json);
        assert_eq!(found[0]["name"].as_str(), Some("acn.bogus.thing"));
        assert_eq!(found[0]["file"].as_str(), Some("crates/fix/src/lib.rs"));
    }
}

/// Cites: TRC-20
#[test]
fn self_host_the_attribute_page_is_current_and_no_crate_emits_an_unlisted_name() {
    let run = xtask_at(&repo_root(), &["docs-inventory", "--check"]);
    assert!(run.ok(), "{}", run.json);
    assert!(
        repo_root()
            .join("docs/generated/acn-attributes.md")
            .is_file()
    );
    assert_eq!(
        run.json["unlisted_attributes"].as_array().map(Vec::len),
        Some(0)
    );
}

/// Each of these got past the first version of the emission check.
///
/// Cites: TRC-20, CON-29
#[test]
fn built_names_odd_spellings_nested_test_modules_and_unlisted_options_are_caught() {
    for (source, name) in [
        (
            "pub fn f(x: u8) -> String { format!(\"acn.missed.{x}\") }\n",
            "acn.missed.{x}",
        ),
        (
            "pub const P: &str = concat!(\"acn.\", \"built\");\n",
            "acn.",
        ),
        (
            "pub const N: &str = \"acn.Missed.Uppercase\";\n",
            "acn.Missed.Uppercase",
        ),
        (
            "pub const N: &str = \"acn.missed-hyphen\";\n",
            "acn.missed-hyphen",
        ),
        (
            "pub const O: &str = \"opt.not_an_option\";\n",
            "opt.not_an_option",
        ),
    ] {
        let dir = fixture_with_schema(source);
        let run = xtask_at(dir.path(), &["docs-inventory"]);
        assert!(!run.ok(), "{source}: {}", run.json);
        let names: Vec<&str> = run.json["unlisted_attributes"]
            .as_array()
            .expect("unlisted_attributes")
            .iter()
            .map(|u| u["name"].as_str().expect("name"))
            .collect();
        assert_eq!(names, [name], "{source}");
    }
    // A listed option is fine, and so is prose that merely mentions a name.
    let ok = "pub const O: &str = \"opt.stall_threshold_ms\";\npub const M: &str = \"see acn.session for details\";\n";
    assert!(xtask_at(fixture_with_schema(ok).path(), &["docs-inventory"]).ok());

    // `src/tests/` is source, not an integration-test directory.
    let dir = fixture_with_schema("pub mod tests;\n");
    let nested = dir.path().join("crates/fix/src/tests");
    fs::create_dir_all(&nested).expect("mkdir");
    fs::write(
        nested.join("mod.rs"),
        "pub const N: &str = \"acn.hidden.in_src_tests\";\n",
    )
    .expect("write");
    let run = xtask_at(dir.path(), &["docs-inventory"]);
    assert!(!run.ok(), "{}", run.json);
}

/// Cites: TRC-36
#[test]
fn docs_inventory_fails_on_a_missing_or_wrong_coverage_mapping_and_renders_a_good_one() {
    let dir = fixture_with_schema("pub fn nothing() {}\n");
    let spec = "specs/010-trace-schema.md";
    fs::copy(repo_root().join(spec), dir.path().join(spec)).expect("copy spec");
    let mapping = dir.path().join("docs/report/coverage.toml");
    // No mapping at all.
    let run = xtask_at(dir.path(), &["docs-inventory"]);
    assert!(!run.ok(), "{}", run.json);
    assert!(
        run.json.to_string().contains("coverage.toml is missing"),
        "{}",
        run.json
    );
    // A mapping with a key dropped, and one naming a column the views lack.
    let good = fs::read_to_string(repo_root().join("docs/report/coverage.toml")).expect("mapping");
    fs::create_dir_all(mapping.parent().expect("parent")).expect("mkdir");
    for (bad, needle) in [
        (
            good.replacen("key = \"h.ttft\"", "key = \"h.ttft_typo\"", 1),
            "not an Appendix A key",
        ),
        (
            good.replacen("column = \"ttft_ns\"", "column = \"ttft_ms\"", 1),
            "does not define",
        ),
    ] {
        fs::write(&mapping, bad).expect("write");
        let run = xtask_at(dir.path(), &["docs-inventory", "--check"]);
        assert!(!run.ok(), "{}", run.json);
        assert!(
            run.json.to_string().contains(needle),
            "{needle}: {}",
            run.json
        );
    }
    fs::write(&mapping, good).expect("write");
    let run = xtask_at(dir.path(), &["docs-inventory"]);
    assert!(run.ok(), "{}", run.json);
    let page =
        fs::read_to_string(dir.path().join("docs/generated/report-coverage.md")).expect("page");
    assert!(
        page.contains("| `h.ttft` | column | `call.ttft_ns` |"),
        "{page}"
    );
}
