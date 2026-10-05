//! HYP-1..3, HYP-5..9: strict parsing at every level, the tables and the stem
//! rule, status by location and by a verified `env-hash.json`, the hash, domains
//! and reserved words, the candidate allowances, and the control and design rules.

#![allow(clippy::unwrap_used, clippy::expect_used, clippy::panic)] // CON-19: tests are exempt

mod common;

use acn_hyp::Status;
use acn_hyp::file::{Control, Domain, Tolerance};
use common::{BASE, candidate, err, frozen, frozen_text, load_at, root, with_predicate};

/// Cites: HYP-1
#[test]
fn every_table_rejects_unknown_keys_and_names_the_key_path() {
    let (h, _d) = candidate(BASE, "t1");
    assert_eq!(h.unwrap().id(), "t1");
    let add = |after: &str, line: &str| BASE.replace(after, &format!("{after}\n{line}"));
    for (bad, key) in [
        (add("title = \"a test\"", "x = 1"), "poc.x"),
        (add("statement = \"s\"", "x = 1"), "hypothesis.x"),
        (
            BASE.replace(
                "knob = { kind = \"bool\" }",
                "knob = { kind = \"bool\", x = 1 }",
            ),
            "varies.knob.x",
        ),
        (
            add("secondary = [\"ttft_p50_ms\", \"ttft_p99_ms\"]", "x = 1"),
            "measures.x",
        ),
        (add("description = \"defaults\"", "x = 1"), "control.x"),
        (add("twin_required = false", "x = 1"), "design.x"),
        (
            add(
                "twin_required = false",
                "pins = { scenario = [], workload = [], models = {}, x = 1 }",
            ),
            "design.pins.x",
        ),
        (add("[falsifier]", "x = 1"), "falsifier.x"),
        (add("outcome = \"pass\"", "x = 1"), "expected.x"),
        (format!("{BASE}\n[extra]\nx = 1\n"), "extra"),
    ] {
        let e = candidate(&bad, "t1").0.unwrap_err();
        assert!(e.message.contains("unknown field"), "{key}: {e}");
        assert_eq!(e.key.as_deref(), Some(key), "{e}");
    }
    let e = candidate(
        &BASE.replace("replicates = 4", "replicates = \"four\""),
        "t1",
    )
    .0
    .unwrap_err();
    assert_eq!(e.key.as_deref(), Some("design.replicates"));
    assert!(e.message.contains("invalid type"), "{e}");
    let e = candidate(&BASE.replace("[expected]\noutcome = \"pass\"\n", ""), "t1")
        .0
        .unwrap_err();
    assert!(e.message.contains("missing field `expected`"), "{e}");
}

/// Cites: HYP-2
#[test]
fn the_tables_poc_and_stem_rules_hold() {
    assert!(candidate(BASE, "t1-slug").0.is_ok(), "`<id>-<slug>`");
    assert!(err(candidate(BASE, "other").0).contains("file stem `other`"));
    let bad_id = BASE.replace("id = \"t1\"", "id = \"T-1\"");
    assert!(err(candidate(&bad_id, "T-1").0).contains("not an identifier"));
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
}

/// Cites: HYP-2
#[test]
fn a_frozen_file_names_a_real_spec_and_a_unique_id() {
    let f = frozen_text(BASE);
    assert!(frozen(&f, "t1").0.is_ok(), "listed in specs/README.md");
    // A spec that exists but is not listed is accepted too.
    let other = f.replace("specs/100-x.md", "specs/200-y.md");
    let dir = root(&[("hypotheses/t1.toml", &other), ("specs/200-y.md", "# Y\n")]);
    assert_eq!(
        load_at(dir.path(), "hypotheses/t1.toml").unwrap().status(),
        Status::Frozen
    );
    for (spec, needle) in [
        ("specs/999-nope.md", "neither exists nor is listed"),
        ("specs/0-x.md", "neither exists nor is listed"),
        ("md", "is not `specs/<name>.md`"),
        ("/etc/hosts", "is not `specs/<name>.md`"),
        ("specs/../../etc/hosts.md", "is not `specs/<name>.md`"),
        ("specs/README", "is not `specs/<name>.md`"),
    ] {
        let e = err(frozen(&f.replace("specs/100-x.md", spec), "t1").0);
        assert!(e.contains(needle), "{spec}: {e}");
    }
    let nospec = f.replace("spec = \"specs/100-x.md\"\n", "");
    assert!(err(frozen(&nospec, "t1").0).contains("names its POC spec"));
    // Ids are unique across hypotheses/, subdirectories included.
    let dir = root(&[
        ("hypotheses/t1.toml", &f),
        ("hypotheses/sub/t1-copy.toml", &f),
    ]);
    let e = err(load_at(dir.path(), "hypotheses/t1.toml"));
    assert!(e.contains("is also the id of"), "{e}");
    // A file there that does not parse is no excuse.
    let dir = root(&[
        ("hypotheses/t1.toml", &f),
        ("hypotheses/broken.toml", "not = = toml"),
    ]);
    let e = err(load_at(dir.path(), "hypotheses/t1.toml"));
    assert!(e.contains("cannot check that `t1` is unique"), "{e}");
}

/// Cites: HYP-3, CON-28
#[test]
fn status_is_decided_by_location_and_by_a_record_that_matches() {
    let f = frozen_text(BASE);
    let (h, _d) = frozen(&f, "t1");
    assert_eq!(h.unwrap().status(), Status::Frozen);
    assert_eq!(
        candidate(&f, "t1").0.unwrap().status(),
        Status::Candidate,
        "outside a root"
    );
    // A byte copy at a path the record does not list is a candidate...
    let dir = root(&[("hypotheses/t1.toml", &f)]);
    std::fs::write(dir.path().join("hypotheses/t1-old.toml"), &f).unwrap();
    let copy = load_at(dir.path(), "hypotheses/t1-old.toml").unwrap();
    assert_eq!(copy.status(), Status::Candidate);
    assert!(
        copy.warnings()
            .iter()
            .any(|w| w.contains("does not match the frozen set"))
    );
    // ...and, the frozen set no longer matching its record, so is everything there.
    assert_eq!(
        load_at(dir.path(), "hypotheses/t1.toml").unwrap().status(),
        Status::Candidate
    );
    // Edited after recording: a candidate.
    let dir = root(&[("hypotheses/t1.toml", &f)]);
    std::fs::write(
        dir.path().join("hypotheses/t1.toml"),
        format!("{f}# edited\n"),
    )
    .unwrap();
    assert_eq!(
        load_at(dir.path(), "hypotheses/t1.toml").unwrap().status(),
        Status::Candidate
    );
    // A forged record that lists the file but does not match the frozen set
    // freezes nothing.
    let dir = root(&[("hypotheses/t1.toml", &f)]);
    let rec = std::fs::read_to_string(dir.path().join("env-hash.json")).unwrap();
    let mut v: serde_json::Value = serde_json::from_str(&rec).unwrap();
    v["env_hash"] = serde_json::json!("0".repeat(64));
    std::fs::write(dir.path().join("env-hash.json"), v.to_string()).unwrap();
    assert_eq!(
        load_at(dir.path(), "hypotheses/t1.toml").unwrap().status(),
        Status::Candidate
    );
    // The root is the current directory's, never the file's own (CON-28): a
    // nested kit beside the file freezes nothing when loaded from outside it.
    let outer = root(&[]);
    let kit = outer.path().join("lab/evil");
    for d in acn_trace::env::FROZEN_SET {
        std::fs::create_dir_all(kit.join(d)).unwrap();
    }
    std::fs::create_dir_all(kit.join("specs")).unwrap();
    std::fs::write(kit.join("specs/README.md"), "| 100-x.md |\n").unwrap();
    std::fs::write(kit.join("hypotheses/t1.toml"), &f).unwrap();
    common::record(&kit);
    let p = kit.join("hypotheses/t1.toml");
    assert_eq!(
        acn_hyp::load_in(&p, outer.path()).unwrap().status(),
        Status::Candidate
    );
    assert_eq!(
        acn_hyp::load_in(&p, &kit).unwrap().status(),
        Status::Frozen,
        "from inside, it is its own kit"
    );
    // [poc].status must agree, both ways.
    let says = |s: &str| {
        f.replace(
            "title = \"a test\"",
            &format!("title = \"a test\"\nstatus = \"{s}\""),
        )
    };
    assert!(err(candidate(&says("frozen"), "t1").0).contains("by location and record"));
    assert_eq!(
        frozen(&says("frozen"), "t1").0.unwrap().status(),
        Status::Frozen
    );
    assert!(err(frozen(&says("candidate"), "t1").0).contains("by location and record"));
    assert!(err(candidate(&says("bogus"), "t1").0).contains("by location and record"));
}

/// Cites: HYP-5
#[test]
fn the_hash_is_of_the_bytes_comments_included() {
    let (h, _d) = candidate(BASE, "t1");
    assert_eq!(
        h.unwrap().hash().to_hex(),
        blake3::hash(BASE.as_bytes()).to_hex().to_string()
    );
    let (h2, _d) = candidate(&format!("# a comment\n{BASE}"), "t1");
    assert_ne!(
        h2.unwrap().hash().to_hex(),
        blake3::hash(BASE.as_bytes()).to_hex().to_string()
    );
}

fn varies(line: &str) -> String {
    BASE.replace(
        "mode = { kind = \"enum\", values = [\"fast\", \"slow\"] }",
        &format!("mode = {{ kind = \"enum\", values = [\"fast\", \"slow\"] }}\n{line}"),
    )
}

fn random(s: String) -> String {
    s.replace("search = \"grid\"", "search = \"random\"")
}

/// Cites: HYP-6
#[test]
fn domains_are_declared_finite_unique_and_unreserved() {
    let ok = |s: &str| candidate(s, "t1").0.unwrap();
    let h = ok(&random(varies(
        "r = { kind = \"range\", min = 0, max = 2.5, levels = [0, 1.5] }",
    )));
    assert_eq!(
        h.params()["r"].domain,
        Domain::Range {
            min: 0.0,
            max: 2.5,
            levels: Some(vec![0.0, 1.5])
        }
    );
    // Integers stay integers, beyond 2^53 too.
    let h = ok(&varies(
        "r = { kind = \"int_range\", min = 9007199254740993, max = 9007199254740995, levels = [9007199254740993] }",
    ));
    assert_eq!(
        h.params()["r"].domain,
        Domain::IntRange {
            min: 9007199254740993,
            max: 9007199254740995,
            levels: Some(vec![9007199254740993])
        }
    );
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
            "r = { kind = \"range\", min = 0, max = inf, levels = [0] }",
            "finite number",
        ),
        (
            "r = { kind = \"range\", min = nan, max = 1, levels = [0] }",
            "finite number",
        ),
        (
            "r = { kind = \"range\", min = 0, max = 1, levels = [nan] }",
            "finite number",
        ),
        (
            "r = { kind = \"range\", min = 0, max = \"x\", levels = [0] }",
            "finite number",
        ),
        (
            "r = { kind = \"int_range\", min = 0, max = 4, levels = [1.5] }",
            "integers",
        ),
        (
            "r = { kind = \"int_range\", min = 0.5, max = 4, levels = [1] }",
            "integer",
        ),
        ("r = { kind = \"enum\", values = [] }", "at least one"),
        (
            "r = { kind = \"enum\", values = [\"fast\"] }",
            "unique across parameters",
        ),
        (
            "r = { kind = \"enum\", values = [\"a\"], levels = [1] }",
            "takes no `levels`",
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
    // Quantity names are not reserved words either.
    let e = err(candidate(
        &BASE.replace(
            "secondary = [\"ttft_p50_ms\", \"ttft_p99_ms\"]",
            "secondary = [\"cells\"]",
        ),
        "t1",
    )
    .0);
    assert!(e.contains("reserved"), "{e}");
}

/// Cites: HYP-6, HYP-8, HYP-12
#[test]
fn pooled_false_takes_effect_and_needs_a_finite_set_of_values() {
    let h = candidate(&varies("r = { kind = \"bool\", pooled = false }"), "t1")
        .0
        .unwrap();
    assert!(!h.params()["r"].pooled);
    assert!(h.params()["knob"].pooled);
    for line in [
        "r = { kind = \"range\", min = 0, max = 4, levels = [1, 2], pooled = false }",
        "r = { kind = \"int_range\", min = 0, max = 4, levels = [1, 2], pooled = false }",
    ] {
        assert!(
            !candidate(&varies(line), "t1").0.unwrap().params()["r"].pooled,
            "{line}"
        );
    }
    let e = err(candidate(
        &random(varies(
            "r = { kind = \"range\", min = 0, max = 1, pooled = false }",
        )),
        "t1",
    )
    .0);
    assert!(e.contains("finite, declared set"), "{e}");
    // `provider` is never pooled, so it needs a finite domain too (CON-26).
    let h = candidate(
        &varies("provider = { kind = \"enum\", values = [\"a\", \"b\"] }"),
        "t1",
    )
    .0
    .unwrap();
    assert!(!h.params()["provider"].pooled);
    let e = err(candidate(
        &random(varies("provider = { kind = \"range\", min = 0, max = 1 }")),
        "t1",
    )
    .0);
    assert!(e.contains("finite, declared set"), "{e}");
    // A non-pooled parameter is fixed by neither a control nor a selector.
    let np = varies("r = { kind = \"bool\", pooled = false }");
    assert!(
        err(candidate(
            &np.replace("config = { knob = false }", "config = { r = true }"),
            "t1"
        )
        .0)
        .contains("not pooled")
    );
    let e = err(candidate(
        &np.replace(
            "predicate = \"max_over_knobs(abs(effect(cached_token_ratio))) < noise_floor(cached_token_ratio, control)\"",
            "predicate = \"max_over_knobs(cached_token_ratio(r = true, fast)) < 1\"",
        ),
        "t1",
    )
    .0);
    assert!(e.contains("not pooled"), "{e}");
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
        h.warnings()
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
fn a_control_is_a_nonempty_config_within_domains_or_a_workload() {
    let h = candidate(BASE, "t1").0.unwrap();
    assert!(matches!(&h.control(), Control::Config(c) if c.len() == 1));
    let wl = BASE.replace(
        "config = { knob = false }",
        "workload = \"plain_rpc\"\ninherits = [\"mode\"]",
    );
    assert!(matches!(
        candidate(&wl, "t1").0.unwrap().control(),
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
            BASE.replace(
                "config = { knob = false }",
                "config = { knob = false }\ninherits = [\"mode\"]",
            ),
            "belongs to a `workload` control",
        ),
        (
            BASE.replace("config = { knob = false }", "config = {}"),
            "at least one parameter",
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
            .warnings()
            .iter()
            .any(|w| w.contains("no control"))
    );
    assert!(err(frozen(&frozen_text(&none), "t1").0).contains("carries `config` or `workload`"));
}

/// Cites: HYP-9
#[test]
fn the_design_rules_hold_and_a_frozen_file_is_stricter() {
    let d = |from: &str, to: &str| BASE.replace(from, to);
    let t = |line: &str| {
        d(
            "twin_required = false",
            &format!("twin_required = false\n{line}"),
        )
    };
    assert_eq!(candidate(BASE, "t1").0.unwrap().design().replicates, 4);
    for (bad, needle) in [
        (d("search = \"grid\"", "search = \"anneal\""), "is not grid"),
        (d("replicates = 4", "replicates = 5"), "even integer"),
        (d("replicates = 4", "replicates = 2"), "even integer"),
        (t("seeds = \"fresh\""), "only `derived`"),
        (t("seed = -1"), "2^63"),
        (t("backends = [\"gpu\"]"), "mockllm or real-api"),
        (t("min_providers_for_verdict = 1"), "`provider` parameter"),
        (
            t("sim_live_tolerance = { widgets = 0.1 }"),
            "not a [measures] quantity",
        ),
        (
            t("sim_live_tolerance = { cached_token_ratio = 0.7 }"),
            "at most 0.5",
        ),
        (
            t("sim_live_tolerance = { cached_token_ratio = -0.1 }"),
            "greater than zero",
        ),
        (
            t("sim_live_tolerance = { cached_token_ratio = { abs = 0 } }"),
            "greater than zero",
        ),
        (
            t("sim_live_tolerance = { cached_token_ratio = { abs = 1, rel = 0.1 } }"),
            "exactly `{ abs = x }`",
        ),
        (
            t("pins = { scenario = [\"xyz\"], workload = [], models = {} }"),
            "BLAKE3 hex digest",
        ),
        (
            t("pins = { scenario = [], workload = [], models = { zzz = \"m\" } }"),
            "not a provider value",
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
        h.design().sim_live_tolerance["ttft_p50_ms"],
        Tolerance::Absolute(3.0)
    );
    assert_eq!(
        candidate(&t("seed = 9"), "t1").0.unwrap().design().seed,
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
    assert!(
        err(frozen(&f.replace("search = \"grid\"", "search = \"random\""), "t1").0)
            .contains("uses `grid`")
    );
    // The provider minimum lies between 1 and the providers.
    let p = varies("provider = { kind = \"enum\", values = [\"a\", \"b\"] }");
    let mp = |m: u32| {
        p.replace(
            "twin_required = false",
            &format!("twin_required = false\nmin_providers_for_verdict = {m}"),
        )
    };
    assert_eq!(
        candidate(&mp(2), "t1")
            .0
            .unwrap()
            .design()
            .min_providers_for_verdict,
        Some(2)
    );
    for m in [0, 3] {
        assert!(
            err(candidate(&mp(m), "t1").0).contains("between 1 and the 2"),
            "{m}"
        );
    }
    let pins = p.replace(
        "twin_required = false",
        &format!(
            "twin_required = false\npins = {{ scenario = [\"{}\"], workload = [], models = {{ a = \"m-1\" }} }}",
            "b".repeat(64)
        ),
    );
    assert_eq!(
        candidate(&pins, "t1")
            .0
            .unwrap()
            .design()
            .pins
            .clone()
            .unwrap()
            .models["a"],
        "m-1"
    );
}
