//! HYP-1..3, HYP-5..9: strict parsing, the tables and the stem rule, status by
//! location and by `env-hash.json`, the hash, domains and reserved words, the
//! candidate allowances, and the control and design rules.

#![allow(clippy::unwrap_used, clippy::expect_used, clippy::panic)] // CON-19: tests are exempt

mod common;

use acn_hyp::Status;
use acn_hyp::file::{Control, Domain, Tolerance};
use common::{BASE, candidate, err, frozen, frozen_text, root, with_predicate};

/// Cites: HYP-1
#[test]
fn a_file_parses_strictly_and_a_failure_names_its_key() {
    let (h, _d) = candidate(BASE, "t1");
    let h = h.unwrap();
    assert_eq!(h.id, "t1");
    for (bad, key, why) in [
        (
            BASE.replace("[hypothesis]", "[hypothesis]\nnote = \"x\""),
            "hypothesis.note",
            "unknown field",
        ),
        (
            BASE.replace("twin_required = false", "twin_required = false\nbudget = 3"),
            "design.budget",
            "unknown field",
        ),
        (
            BASE.replace(
                "knob = { kind = \"bool\" }",
                "knob = { kind = \"bool\", default = true }",
            ),
            "varies.knob.default",
            "unknown field",
        ),
        (
            BASE.replace("replicates = 4", "replicates = \"four\""),
            "design.replicates",
            "invalid type",
        ),
        (
            format!("{BASE}\n[extra]\nx = 1\n"),
            "extra",
            "unknown field",
        ),
        (
            BASE.replace("[expected]\noutcome = \"pass\"\n", ""),
            "",
            "missing field `expected`",
        ),
    ] {
        let e = err(candidate(&bad, "t1").0);
        assert!(e.contains(why), "{key}: {e}");
        assert!(e.contains(key), "the key path `{key}` is named: {e}");
    }
}

/// Cites: HYP-2
#[test]
fn the_tables_poc_and_stem_rules_hold() {
    assert!(candidate(BASE, "t1-slug").0.is_ok(), "`<id>-<slug>`");
    assert!(err(candidate(BASE, "other").0).contains("file stem `other`"));
    let sup = BASE.replace(
        "title = \"a test\"",
        "title = \"a test\"\nsupersedes = \"t0@abc\"",
    );
    assert!(err(candidate(&sup, "t1").0).contains("<id>@<hypothesis_hash>"));
    let sup = BASE.replace(
        "title = \"a test\"",
        &format!("title = \"a test\"\nsupersedes = \"t0@{}\"", "a".repeat(64)),
    );
    assert!(candidate(&sup, "t1").0.is_ok());
    // A frozen file names a spec that exists or is listed.
    let f = frozen_text(BASE);
    assert!(frozen(&f, "t1").0.is_ok());
    let unlisted = f.replace("specs/100-x.md", "specs/999-nope.md");
    assert!(err(frozen(&unlisted, "t1").0).contains("neither exists nor is listed"));
    let nospec = f.replace("spec = \"specs/100-x.md\"\n", "");
    assert!(err(frozen(&nospec, "t1").0).contains("names its POC spec"));
    // An id is unique across hypotheses/.
    let twin = f.clone();
    let dir = root(&[
        ("hypotheses/t1.toml", &f),
        ("hypotheses/t1-copy.toml", &twin),
    ]);
    let e = err(acn_hyp::load(&dir.path().join("hypotheses/t1.toml")));
    assert!(e.contains("is also the id of"), "{e}");
}

/// Cites: HYP-3
#[test]
fn status_is_decided_by_location_and_by_the_record() {
    let f = frozen_text(BASE);
    let (h, _d) = frozen(&f, "t1");
    assert_eq!(h.unwrap().status, Status::Frozen);
    assert_eq!(
        candidate(&f, "t1").0.unwrap().status,
        Status::Candidate,
        "outside a root"
    );
    // Under hypotheses/ but edited after recording: a candidate, and its
    // candidate rules apply.
    let dir = root(&[("hypotheses/t1.toml", &f)]);
    let path = dir.path().join("hypotheses/t1.toml");
    std::fs::write(&path, format!("{f}# edited\n")).unwrap();
    assert_eq!(acn_hyp::load(&path).unwrap().status, Status::Candidate);
    // [poc].status must agree.
    let claims = f.replace(
        "title = \"a test\"",
        "title = \"a test\"\nstatus = \"frozen\"",
    );
    assert!(err(candidate(&claims, "t1").0).contains("by location and record"));
    assert_eq!(frozen(&claims, "t1").0.unwrap().status, Status::Frozen);
}

/// Cites: HYP-5
#[test]
fn the_hash_is_of_the_bytes_comments_included() {
    let (h, _d) = candidate(BASE, "t1");
    assert_eq!(
        h.unwrap().hash.to_hex(),
        blake3::hash(BASE.as_bytes()).to_hex().to_string()
    );
    let (h2, _d) = candidate(&format!("# a comment\n{BASE}"), "t1");
    assert_ne!(
        h2.unwrap().hash.to_hex(),
        blake3::hash(BASE.as_bytes()).to_hex().to_string()
    );
}

fn varies(line: &str) -> String {
    BASE.replace(
        "mode = { kind = \"enum\", values = [\"fast\", \"slow\"] }",
        &format!("mode = {{ kind = \"enum\", values = [\"fast\", \"slow\"] }}\n{line}"),
    )
}

/// Cites: HYP-6
#[test]
fn domains_are_declared_finite_unique_and_unreserved() {
    let ok = |s: &str| candidate(s, "t1").0.unwrap();
    let h = ok(
        &varies("r = { kind = \"range\", min = 0, max = 2.5, levels = [0, 1.5] }")
            .replace("search = \"grid\"", "search = \"random\""),
    );
    assert_eq!(
        h.params["r"].domain,
        Domain::Range {
            min: 0.0,
            max: 2.5,
            levels: Some(vec![0.0, 1.5])
        }
    );
    let h = ok(&varies(
        "provider = { kind = \"enum\", values = [\"a\", \"b\"] }",
    ));
    assert!(
        !h.params["provider"].pooled,
        "provider is never pooled (CON-26)"
    );
    assert!(h.params["knob"].pooled);
    for (line, needle) in [
        (
            "r = { kind = \"range\", min = 3, max = 1, levels = [2] }",
            "exceeds max",
        ),
        (
            "r = { kind = \"range\", min = 0, max = 1 }",
            "grid search needs `levels`",
        ),
        (
            "r = { kind = \"range\", min = 0, max = 1, levels = [] }",
            "empty",
        ),
        (
            "r = { kind = \"range\", min = 0, max = 1, levels = [2] }",
            "outside",
        ),
        (
            "r = { kind = \"range\", min = 0, max = 1, levels = [1, 1] }",
            "listed twice",
        ),
        (
            "r = { kind = \"int_range\", min = 0, max = 4, levels = [1.5] }",
            "integers",
        ),
        (
            "r = { kind = \"int_range\", min = 0.5, max = 4, levels = [1] }",
            "integer",
        ),
        (
            "r = { kind = \"range\", min = 0, max = \"x\", levels = [0] }",
            "finite number",
        ),
        ("r = { kind = \"enum\", values = [] }", "at least one"),
        (
            "r = { kind = \"enum\", values = [\"fast\"] }",
            "unique across parameters",
        ),
        (
            "r = { kind = \"bool\", values = [\"x\"] }",
            "takes no `values`",
        ),
        ("r = { kind = \"set\" }", "is not bool, enum"),
        ("and = { kind = \"bool\" }", "reserved"),
        (
            "r = { kind = \"enum\", values = [\"control\"] }",
            "reserved",
        ),
    ] {
        let e = err(candidate(&varies(line), "t1").0);
        assert!(e.contains(needle), "{line}: {e}");
    }
    // pooled = false only where the slices are finite and declared.
    let e = err(candidate(
        &varies("r = { kind = \"range\", min = 0, max = 1, pooled = false }")
            .replace("search = \"grid\"", "search = \"random\""),
        "t1",
    )
    .0);
    assert!(e.contains("pooled = false"), "{e}");
    assert!(
        candidate(&varies("r = { kind = \"bool\", pooled = false }"), "t1")
            .0
            .is_ok()
    );
}

/// Cites: HYP-7
#[test]
fn every_measured_quantity_resolves_in_a_frozen_file_and_warns_in_a_candidate() {
    let unknown = BASE.replace(
        "secondary = [\"ttft_p50_ms\", \"ttft_p99_ms\"]",
        "secondary = [\"widgets\"]",
    );
    let h = candidate(&unknown, "t1").0.unwrap();
    assert!(
        h.warnings
            .iter()
            .any(|w| w.contains("`widgets` is not in the quantity table"))
    );
    let e = err(frozen(&frozen_text(&unknown), "t1").0);
    assert!(e.contains("resolves to no quantity"), "{e}");
    let e = err(candidate(&with_predicate("max_over_knobs(widgets) < 1"), "t1").0);
    assert!(e.contains("not a [measures] quantity"), "{e}");
    assert!(
        err(candidate(
            &BASE.replace("primary = [\"cached_token_ratio\"]", "primary = []"),
            "t1"
        )
        .0)
        .contains("at least one")
    );
}

/// Cites: HYP-8, CON-18
#[test]
fn a_control_is_a_config_within_domains_or_a_workload() {
    let h = candidate(BASE, "t1").0.unwrap();
    assert!(matches!(&h.control, Control::Config(c) if c.len() == 1));
    let wl = BASE.replace(
        "config = { knob = false }",
        "workload = \"plain_rpc\"\ninherits = [\"mode\"]",
    );
    assert!(matches!(
        candidate(&wl, "t1").0.unwrap().control,
        Control::Workload { .. }
    ));
    for (bad, needle) in [
        (
            BASE.replace(
                "config = { knob = false }",
                "config = { knob = false }\nworkload = \"x\"",
            ),
            "not both",
        ),
        (
            BASE.replace("config = { knob = false }", "config = { speed = 1 }"),
            "not a [varies] parameter",
        ),
        (
            BASE.replace(
                "config = { knob = false }",
                "config = { mode = \"medium\" }",
            ),
            "outside the domain",
        ),
        (
            BASE.replace(
                "config = { knob = false }",
                "workload = \"x\"\ninherits = [\"nope\"]",
            ),
            "not a [varies] parameter",
        ),
    ] {
        let e = err(candidate(&bad, "t1").0);
        assert!(e.contains(needle), "{needle}: {e}");
    }
    let none = BASE.replace("config = { knob = false }\n", "");
    assert!(
        candidate(&none, "t1")
            .0
            .unwrap()
            .warnings
            .iter()
            .any(|w| w.contains("no control"))
    );
    assert!(err(frozen(&frozen_text(&none), "t1").0).contains("carries `config` or `workload`"));
    let np = varies("provider = { kind = \"enum\", values = [\"a\", \"b\"] }")
        .replace("config = { knob = false }", "config = { provider = \"a\" }");
    assert!(err(candidate(&np, "t1").0).contains("not pooled"));
}

/// Cites: HYP-9
#[test]
fn the_design_rules_hold_and_a_frozen_file_is_stricter() {
    let d = |from: &str, to: &str| BASE.replace(from, to);
    let h = candidate(BASE, "t1").0.unwrap();
    assert_eq!(h.design.replicates, 4);
    for (bad, needle) in [
        (d("search = \"grid\"", "search = \"anneal\""), "is not grid"),
        (d("replicates = 4", "replicates = 5"), "even integer"),
        (d("replicates = 4", "replicates = 2"), "even integer"),
        (
            d(
                "twin_required = false",
                "twin_required = false\nseeds = \"fresh\"",
            ),
            "only `derived`",
        ),
        (
            d("twin_required = false", "twin_required = false\nseed = -1"),
            "2^63",
        ),
        (
            d(
                "twin_required = false",
                "twin_required = false\nbackends = [\"gpu\"]",
            ),
            "mockllm or real-api",
        ),
        (
            d(
                "twin_required = false",
                "twin_required = false\nmin_providers_for_verdict = 1",
            ),
            "`provider` parameter",
        ),
        (
            d(
                "twin_required = false",
                "twin_required = false\nsim_live_tolerance = { widgets = 0.1 }",
            ),
            "not a [measures] quantity",
        ),
        (
            d(
                "twin_required = false",
                "twin_required = false\nsim_live_tolerance = { cached_token_ratio = 0.7 }",
            ),
            "at most 0.5",
        ),
        (
            d(
                "twin_required = false",
                "twin_required = false\nsim_live_tolerance = { cached_token_ratio = { abs = 0 } }",
            ),
            "greater than zero",
        ),
        (
            d(
                "twin_required = false",
                "twin_required = false\npins = { scenario = [\"xyz\"], workload = [], models = {} }",
            ),
            "BLAKE3 hex digest",
        ),
        (
            d("twin_required = false", "twin_required = true"),
            "has no tolerance",
        ),
        (
            d("outcome = \"pass\"", "outcome = \"maybe\""),
            "pass or fail",
        ),
    ] {
        let e = err(candidate(&bad, "t1").0);
        assert!(e.contains(needle), "{needle}: {e}");
    }
    let twin = d(
        "twin_required = false",
        "twin_required = true\nsim_live_tolerance = { cached_token_ratio = 0.05, ttft_p50_ms = { abs = 3 } }",
    );
    let h = candidate(&twin, "t1").0.unwrap();
    assert_eq!(
        h.design.sim_live_tolerance["ttft_p50_ms"],
        Tolerance::Absolute(3.0)
    );
    assert_eq!(
        candidate(
            &d("twin_required = false", "twin_required = false\nseed = 9"),
            "t1"
        )
        .0
        .unwrap()
        .design
        .seed,
        Some(9)
    );
    // Frozen: grid, at least 20 replicates, no seed of its own.
    let f = frozen_text(BASE);
    assert!(
        err(frozen(&f.replace("replicates = 20", "replicates = 10"), "t1").0)
            .contains("at least 20")
    );
    assert!(
        err(frozen(
            &f.replace("twin_required = false", "twin_required = false\nseed = 9"),
            "t1"
        )
        .0)
        .contains("derived from its hash")
    );
    let random = f.replace("search = \"grid\"", "search = \"random\"");
    assert!(err(frozen(&random, "t1").0).contains("uses `grid`"));
    let p = varies("provider = { kind = \"enum\", values = [\"a\", \"b\"] }");
    let ok = p.replace(
        "twin_required = false",
        "twin_required = false\nmin_providers_for_verdict = 2",
    );
    assert_eq!(
        candidate(&ok, "t1")
            .0
            .unwrap()
            .design
            .min_providers_for_verdict,
        Some(2)
    );
    let too_many = p.replace(
        "twin_required = false",
        "twin_required = false\nmin_providers_for_verdict = 3",
    );
    assert!(err(candidate(&too_many, "t1").0).contains("between 1 and the 2"));
}
