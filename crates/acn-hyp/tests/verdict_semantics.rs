//! HYP-20..24, HYP-28 on synthetic bundles: each reason in order, extra
//! replicates ignored, the missing grid cell, Kleene connectives over strict
//! values (a true falsifier with a missing cell fails; a false one does not
//! pass), a zero `noise_floor`, every refusal of HYP-20, labels including the
//! back-dated freeze and `unpinned-inputs`, the twin rule with decision cells and
//! effect divergence, per-provider slices, the half-submitted provider and
//! `partial-providers`, and the fields of `verdict.json`.

#![allow(clippy::unwrap_used, clippy::expect_used, clippy::panic)] // CON-19: tests are exempt

mod common;

use acn_hyp::Hypothesis;
use acn_hyp::read::BundleData;
use acn_hyp::verdict::{V, Verdict, verdict, write};
use acn_trace::identity::Digest;
use common::bundles::{Spec, alt, bundle, engine, extra_replicate, flat};
use common::{BASE, candidate, frozen, frozen_text, with_predicate};

fn load(text: &str) -> Hypothesis {
    candidate(text, "t1").0.unwrap()
}

fn knob(k: bool) -> &'static str {
    if k { "true" } else { "false" }
}

/// Every bundle of one slice: the two controls (`knob = false`, HYP-8) and the
/// four treatment cells, `n` replicates each; treatment = control + `effect`.
fn slice_set(
    h: &Hypothesis,
    n: usize,
    extra: &[(&str, &str)],
    tag: &str,
    with: &dyn Fn(Spec) -> Spec,
    effect: &dyn Fn(bool, &str) -> f64,
) -> Vec<BundleData> {
    let base = alt(n, 0.40, 0.46);
    let mut out = Vec::new();
    for m in ["fast", "slow"] {
        let mut vary = vec![("knob", "false"), ("mode", m)];
        vary.extend_from_slice(extra);
        let s = Spec::new(&format!("{tag}c-{m}"), &vary, "control", base.clone());
        out.push(bundle(h, &with(s)));
    }
    for k in [false, true] {
        for m in ["fast", "slow"] {
            let e = effect(k, m);
            let mut vary = vec![("knob", knob(k)), ("mode", m)];
            vary.extend_from_slice(extra);
            let ratios = base.iter().map(|x| x.map(|x| x + e)).collect();
            let s = Spec::new(&format!("{tag}t-{k}-{m}"), &vary, "treatment", ratios);
            out.push(bundle(h, &with(s)));
        }
    }
    out
}

fn set(h: &Hypothesis, effect: &dyn Fn(bool, &str) -> f64) -> Vec<BundleData> {
    slice_set(h, 4, &[], "", &|s| s, effect)
}

fn big(k: bool, m: &str) -> f64 {
    if k && m == "slow" { 0.3 } else { 0.0 }
}

fn none(_: bool, _: &str) -> f64 {
    0.0
}

fn run(h: &Hypothesis, b: Vec<BundleData>) -> Verdict {
    verdict(h, b, engine()).unwrap()
}

fn refused(h: &Hypothesis, b: Vec<BundleData>) -> String {
    verdict(h, b, engine()).unwrap_err().to_string()
}

fn ids(v: &Verdict) -> Vec<&'static str> {
    v.slices[0].reasons.iter().map(|r| r.id).collect()
}

fn drop_named(mut b: Vec<BundleData>, name: &str) -> Vec<BundleData> {
    let id = Digest::of(format!("run:{name}").as_bytes());
    b.retain(|x| x.run_id != id);
    b
}

fn find<'a>(b: &'a mut [BundleData], name: &str) -> &'a mut BundleData {
    let id = Digest::of(format!("run:{name}").as_bytes());
    b.iter_mut().find(|x| x.run_id == id).unwrap()
}

/// Cites: HYP-21, HYP-11, HYP-13
#[test]
fn a_slice_passes_fails_or_is_inconclusive_for_each_reason_in_order() {
    let h = load(BASE);
    // p4's shape: refuted when no effect clears the noise floor.
    let v = run(&h, set(&h, &big));
    assert_eq!(v.verdict, V::Pass, "{:?}", v.reasons);
    assert!(v.slices[0].reasons.is_empty());
    assert_eq!(run(&h, set(&h, &none)).verdict, V::Fail);
    // control_missing: the `slow` control was never run.
    let v = run(&h, drop_named(set(&h, &big), "c-slow"));
    assert_eq!(v.verdict, V::Inconclusive);
    assert_eq!(ids(&v)[0], "control_missing");
    assert_eq!(
        v.slices[0].reasons[0].refers,
        ["knob=false,mode=slow", "knob=true,mode=slow"]
    );
    // no_evaluated_cell: controls only.
    let only_controls: Vec<BundleData> = set(&h, &big).into_iter().take(2).collect();
    let v = run(&h, only_controls);
    assert_eq!(ids(&v)[..2], ["no_evaluated_cell", "grid_cell_missing"]);
    // incomplete_cell, then undefined_value.
    let mut b = set(&h, &big);
    let t = find(&mut b, "t-true-slow");
    t.sessions.retain(|s| s.replicate != 2);
    let v = run(&h, b);
    assert_eq!(v.verdict, V::Inconclusive);
    assert_eq!(ids(&v), ["incomplete_cell", "undefined_value"]);
    let t = &json(&v)["slices"][0]["cells"][3]["treatment"];
    assert_eq!(
        t["incomplete"],
        serde_json::json!([2]),
        "listed, never skipped"
    );
    // A replicate whose own value is undefined is listed too.
    let mut b = set(&h, &big);
    let c = find(&mut b, "c-fast");
    c.calls[1].input_tokens = None;
    let v = run(&h, b);
    assert_eq!(v.verdict, V::Inconclusive);
    let k = &json(&v)["slices"][0]["controls"][0]["arm"];
    assert_eq!(k["undefined"]["cached_token_ratio"], serde_json::json!([1]));
    // grid_cell_missing: a cell with no runs; with the aggregate it is undefined.
    let v = run(&h, drop_named(set(&h, &big), "t-true-fast"));
    assert_eq!(ids(&v), ["grid_cell_missing", "undefined_value"]);
    assert_eq!(v.slices[0].reasons[0].refers, ["knob=true,mode=fast"]);
    // A zero noise floor: no variation, nothing to compare with.
    let flat_h = h.clone();
    let mut b = set(&flat_h, &big);
    for x in &mut b {
        for c in &mut x.calls {
            c.cache_read_tokens = Some(
                40_000
                    + if x.manifest.params["arms"] == "treatment" {
                        30_000
                    } else {
                        0
                    },
            );
        }
    }
    let v = run(&flat_h, b);
    assert_eq!(ids(&v), ["undefined_value"]);
    assert!(
        v.slices[0].reasons[0]
            .refers
            .iter()
            .any(|e| e.starts_with("noise_floor"))
    );
}

/// Cites: HYP-21, HYP-11, HYP-14
#[test]
fn a_true_falsifier_with_a_missing_cell_fails_and_a_false_one_does_not_pass() {
    let any = load(&with_predicate(
        "effect(cached_token_ratio) > 0.2 at any cells",
    ));
    let missing =
        |h: &Hypothesis, e: &dyn Fn(bool, &str) -> f64| drop_named(set(h, e), "t-true-fast");
    let v = run(&any, missing(&any, &big));
    assert_eq!(
        v.verdict,
        V::Fail,
        "true at (true, slow) refutes despite the missing cell"
    );
    assert_eq!(ids(&v), ["grid_cell_missing"]);
    let v = run(&any, missing(&any, &none));
    assert_eq!(
        v.verdict,
        V::Inconclusive,
        "false where defined is not a pass"
    );
    assert_eq!(ids(&v), ["grid_cell_missing", "undefined_value"]);
    let all = load(&with_predicate(
        "effect(cached_token_ratio) < 0.2 at all cells",
    ));
    let v = run(&all, missing(&all, &big));
    assert_eq!(v.slices[0].eval.value, Some(false));
    assert_eq!(v.verdict, V::Inconclusive, "false past a missing cell");
    assert_eq!(run(&all, set(&all, &big)).verdict, V::Pass);
}

/// Cites: HYP-21, HYP-28
#[test]
fn replicates_beyond_the_design_are_ignored_and_listed() {
    let h = load(BASE);
    let plain = run(&h, set(&h, &big));
    let mut b = set(&h, &big);
    extra_replicate(find(&mut b, "t-true-slow"), 4);
    extra_replicate(find(&mut b, "c-fast"), 9);
    let v = run(&h, b);
    assert_eq!(v.verdict, plain.verdict);
    let ignored = json(&v)["ignored_replicates"].clone();
    assert_eq!(ignored.as_array().unwrap().len(), 2);
    assert!(ignored.to_string().contains("[4]") && ignored.to_string().contains("[9]"));
    // The same evidence: the slices are identical, only the bundle set differs.
    assert_eq!(json(&v)["slices"], json(&plain)["slices"]);
    assert_ne!(v.verdict_id, plain.verdict_id);
}

fn json(v: &Verdict) -> serde_json::Value {
    serde_json::from_str(&v.text()).unwrap()
}

/// Cites: HYP-20
#[test]
fn every_refusal_of_hyp_20() {
    let h = load(BASE);
    let other = load(&BASE.replace("statement = \"s\"", "statement = \"t\""));
    let mut b = set(&h, &big);
    b[3] = bundle(
        &other,
        &Spec::new(
            "x",
            &[("knob", "true"), ("mode", "fast")],
            "treatment",
            flat(4, 0.5),
        ),
    );
    assert!(refused(&h, b).contains("hypothesis.hash"));
    let e = verdict(&h, set(&h, &big), Digest::of(b"other engine"))
        .unwrap_err()
        .to_string();
    assert!(e.contains("engine_hash"), "{e}");
    let b = slice_set(
        &h,
        4,
        &[],
        "",
        &|mut s| {
            if s.name == "t-true-fast" {
                s.build = "other".into();
            }
            s
        },
        &big,
    );
    assert!(refused(&h, b).contains("build_hash"));
    // Two bundles covering one replicate index; disjoint indices are fine.
    let mut b = set(&h, &big);
    b.push(bundle(
        &h,
        &Spec::new(
            "dup",
            &[("knob", "true"), ("mode", "fast")],
            "treatment",
            vec![None, Some(0.4), None, None],
        ),
    ));
    assert!(refused(&h, b).contains("two bundles cover"));
    let mut b = set(&h, &big);
    find(&mut b, "t-true-fast")
        .sessions
        .retain(|s| s.replicate < 2);
    b.push(bundle(
        &h,
        &Spec::new(
            "rest",
            &[("knob", "true"), ("mode", "fast")],
            "treatment",
            vec![None, None, Some(0.40), Some(0.46)],
        ),
    ));
    assert_eq!(
        run(&h, b).verdict,
        V::Pass,
        "one cell's replicates from two bundles"
    );
    // Mock and real backends.
    let b = slice_set(
        &h,
        4,
        &[],
        "",
        &|s| {
            if s.name == "c-fast" {
                s.backend("openai", "gpt")
            } else {
                s
            }
        },
        &big,
    );
    assert!(refused(&h, b).contains("mixes mockllm and real"));
    // A treatment and its control on different scenarios.
    let b = slice_set(
        &h,
        4,
        &[],
        "",
        &|mut s| {
            if s.name == "c-slow" {
                s.scenario = Digest::of(b"other").to_hex();
            }
            s
        },
        &big,
    );
    assert!(refused(&h, b).contains("scenario"));
    // One slice, two models.
    let b = slice_set(
        &h,
        4,
        &[],
        "",
        &|s| {
            if s.name == "t-true-slow" {
                s.backend("mockllm", "mock-auto")
            } else {
                s
            }
        },
        &big,
    );
    assert!(refused(&h, b).contains("backend or model"));
    // Two new_input_tokens methods.
    let b = slice_set(
        &h,
        4,
        &[],
        "",
        &|mut s| {
            if s.name == "c-fast" {
                s.method = "bytes_scaled".into();
            }
            s
        },
        &big,
    );
    assert!(refused(&h, b).contains("new_input_tokens_method"));
    // The parameters of HYP-6.
    let mut b = set(&h, &big);
    b[0].manifest.params.remove("vary.mode");
    assert!(refused(&h, b).contains("no `vary.mode`"));
    let mut b = set(&h, &big);
    b[0].manifest
        .params
        .insert("vary.mode".into(), "medium".into());
    assert!(refused(&h, b).contains("not a value of its domain"));
    let mut b = set(&h, &big);
    b[0].manifest.params.insert("vary.speed".into(), "1".into());
    assert!(refused(&h, b).contains("not a [varies] parameter"));
    // netem beside sim, and live beside netem.
    let b = slice_set(
        &h,
        4,
        &[],
        "",
        &|s| {
            if s.name == "c-fast" {
                s.mode("netem")
            } else {
                s
            }
        },
        &big,
    );
    assert!(refused(&h, b).contains("netem"));
    let b = slice_set(
        &h,
        4,
        &[],
        "",
        &|s| {
            if s.arm == "control" {
                s.mode("netem")
            } else {
                s.mode("live")
            }
        },
        &big,
    );
    assert!(refused(&h, b).contains("netem"));
    assert!(refused(&h, Vec::new()).contains("no bundles"));
}

fn providers_text(min: u32) -> String {
    BASE.replace(
        "mode = { kind = \"enum\", values = [\"fast\", \"slow\"] }",
        "mode = { kind = \"enum\", values = [\"fast\", \"slow\"] }\nprovider = { kind = \"enum\", values = [\"anthropic\", \"openai\"] }",
    )
    .replace(
        "twin_required = false",
        &format!("twin_required = false\nmin_providers_for_verdict = {min}\nbackends = [\"real-api\"]"),
    )
    .replace(
        "[expected]",
        "inconclusive_if = \"replicates < 4 or providers_reported < 2\"\n\n[expected]",
    )
}

fn provider_set(h: &Hypothesis, p: &str, effect: &dyn Fn(bool, &str) -> f64) -> Vec<BundleData> {
    let model = if p == "openai" { "gpt" } else { "claude" };
    slice_set(
        h,
        4,
        &[("provider", p)],
        &format!("{p}-"),
        &|s| s.backend(p, model),
        effect,
    )
}

/// Cites: HYP-20, HYP-24, HYP-23
#[test]
fn providers_are_judged_side_by_side_and_a_missing_one_is_shown() {
    let h = load(&providers_text(2));
    let mut b = provider_set(&h, "anthropic", &big);
    b.extend(provider_set(&h, "openai", &none));
    let v = run(&h, b.clone());
    assert_eq!(v.slices.len(), 2);
    assert_eq!(v.slices[0].key, "provider=anthropic");
    assert_eq!(v.providers["anthropic"], "pass");
    assert_eq!(v.providers["openai"], "fail");
    assert_eq!(v.verdict, V::Fail, "{:?}", v.reasons);
    assert!(!v.labels.contains("mock-gated") && v.labels.contains("exploratory"));
    // vary.provider must be the backend for a real provider.
    let mut wrong = b.clone();
    wrong[0].manifest.backend = "openai".into();
    assert!(refused(&h, wrong).contains("vary.provider"));
    // One provider not run: shown, labelled, and below the minimum.
    let v = run(&h, provider_set(&h, "anthropic", &big));
    assert_eq!(v.providers["openai"], "not_run");
    assert!(v.labels.contains("partial-providers"));
    assert_eq!(v.verdict, V::Inconclusive);
    let r: Vec<&str> = v.reasons.iter().map(|r| r.id).collect();
    assert_eq!(r, ["guard", "providers_below_minimum"]);
    // With a minimum of one, the same set is conclusive, still labelled.
    let h1 = load(&providers_text(1).replace(" or providers_reported < 2", ""));
    let v = run(&h1, provider_set(&h1, "anthropic", &big));
    assert_eq!(v.verdict, V::Pass, "{:?}", v.reasons);
    assert!(v.labels.contains("partial-providers"));
    // Half-submitted: openai was run and found wanting (its control missing).
    let mut b = provider_set(&h, "anthropic", &big);
    b.extend(drop_named(
        provider_set(&h, "openai", &big),
        "openai-c-fast",
    ));
    let v = run(&h, b);
    assert_eq!(v.providers["openai"], "inconclusive");
    assert_eq!(v.verdict, V::Inconclusive);
    assert!(
        v.reasons
            .iter()
            .any(|r| r.id == "slice_inconclusive" && r.refers == ["provider=openai"])
    );
}

fn twin_text(tol: &str) -> String {
    BASE.replace(
        "twin_required = false",
        &format!("twin_required = true\nsim_live_tolerance = {{ cached_token_ratio = {tol} }}"),
    )
}

fn live_of(h: &Hypothesis, sim: &[BundleData], shift: &dyn Fn(&str) -> f64) -> Vec<BundleData> {
    let _ = sim;
    let base = alt(4, 0.40, 0.46);
    let mut out = Vec::new();
    for m in ["fast", "slow"] {
        let r = base
            .iter()
            .map(|x| x.map(|x| x + shift(&format!("c-{m}"))))
            .collect();
        out.push(bundle(
            h,
            &Spec::new(
                &format!("live-c-{m}"),
                &[("knob", "false"), ("mode", m)],
                "control",
                r,
            )
            .mode("live"),
        ));
    }
    for k in [false, true] {
        for m in ["fast", "slow"] {
            let name = format!("t-{k}-{m}");
            let r = base
                .iter()
                .map(|x| x.map(|x| x + big(k, m) + shift(&name)))
                .collect();
            out.push(bundle(
                h,
                &Spec::new(
                    &format!("live-{name}"),
                    &[("knob", knob(k)), ("mode", m)],
                    "treatment",
                    r,
                )
                .mode("live"),
            ));
        }
    }
    out
}

/// Cites: HYP-22, HYP-21, HYP-23
#[test]
fn the_twin_rule_reads_live_bundles_for_divergence_and_twins_the_decision_cells() {
    let h = load(&twin_text("{ abs = 0.03 }"));
    let sim = set(&h, &big);
    // No live bundle: sim-only.
    let v = run(&h, sim.clone());
    assert!(v.labels.contains("sim-only") && v.labels.contains("mock-gated"));
    // Live twins that agree: every decision cell twinned, no twin label.
    let mut b = sim.clone();
    b.extend(live_of(&h, &sim, &|_| 0.01));
    let v = run(&h, b.clone());
    assert_eq!(v.verdict, V::Pass, "{:?}", v.slices[0].reasons);
    assert!(!v.labels.contains("sim-only") && !v.labels.contains("partially-twinned"));
    let j = json(&v);
    let cells = j["slices"][0]["cells"].as_array().unwrap();
    assert!(cells.iter().all(|c| c["twinned"] == true));
    let d = &cells[3]["divergence"]["cached_token_ratio"];
    assert!((d["treatment"].as_f64().unwrap() - 0.01).abs() < 1e-9);
    // The decision cell's twin missing: partially-twinned.
    let decision = drop_named(b.clone(), "live-t-true-slow");
    let v = run(&h, decision);
    assert!(v.labels.contains("partially-twinned"), "{:?}", v.labels);
    // A twin outside the tolerance fails the twin rule.
    let mut far = sim.clone();
    far.extend(live_of(&h, &sim, &|n| {
        if n == "t-false-fast" { 0.05 } else { 0.0 }
    }));
    let v = run(&h, far);
    assert_eq!(v.verdict, V::Inconclusive);
    assert_eq!(ids(&v)[0], "twin_failed");
    assert_eq!(v.slices[0].reasons[0].refers, ["knob=false,mode=fast"]);
    // Arms that agree within the tolerance while the effect does not: the effect
    // moves by 0.04 (> 0.03) though each arm moves by 0.02.
    let mut flip = sim.clone();
    flip.extend(live_of(&h, &sim, &|n| match n {
        "t-false-fast" => -0.02,
        "c-fast" => 0.02,
        _ => 0.0,
    }));
    let v = run(&h, flip);
    assert_eq!(ids(&v)[0], "twin_failed", "{:?}", v.slices[0].reasons);
    // A live cell with no sim cell is listed, and the verdict reads sim only.
    let mut b = drop_named(sim.clone(), "t-true-fast");
    b.extend(live_of(&h, &sim, &|_| 0.0));
    let v = run(&h, b);
    assert_eq!(
        json(&v)["twin_only_cells"],
        serde_json::json!(["knob=true,mode=fast"])
    );
    // Without twin_required, a diverging twin is reported but does not fail.
    let loose = load(&BASE.replace(
        "twin_required = false",
        "twin_required = false\nsim_live_tolerance = { cached_token_ratio = { abs = 0.03 } }",
    ));
    let mut b = set(&loose, &big);
    b.extend(live_of(&loose, &b.clone(), &|n| {
        if n == "t-false-fast" { 0.05 } else { 0.0 }
    }));
    assert_eq!(run(&loose, b).verdict, V::Pass);
}

/// Cites: HYP-23, HYP-9
#[test]
fn labels_follow_the_file_and_the_bundles_and_a_frozen_file_needs_its_seed() {
    let text = frozen_text(BASE);
    let (h, _root) = frozen(&text, "t1");
    let h = h.unwrap();
    let full = |h: &Hypothesis, with: &dyn Fn(Spec) -> Spec| slice_set(h, 20, &[], "", with, &big);
    let v = run(&h, full(&h, &|s| s));
    assert!(!v.labels.contains("exploratory"), "{:?}", v.labels);
    assert!(v.labels.contains("mock-gated") && v.labels.contains("sim-only"));
    // A back-dated freeze: bundles run while the file was a candidate.
    let v = run(
        &h,
        full(&h, &|mut s| {
            s.status = Some("candidate".into());
            s
        }),
    );
    assert!(v.labels.contains("exploratory"));
    // A frozen file's bundles run under its derived seed only.
    let e = refused(
        &h,
        full(&h, &|mut s| {
            if s.name == "c-fast" {
                s.seed = Some(42);
            }
            s
        }),
    );
    assert!(e.contains("seed"), "{e}");
    // A frozen real-api hypothesis with no pins.
    let (u, _root) = frozen(
        &text.replace(
            "twin_required = false",
            "twin_required = false\nbackends = [\"mockllm\", \"real-api\"]",
        ),
        "t1",
    );
    let u = u.unwrap();
    assert!(run(&u, full(&u, &|s| s)).labels.contains("unpinned-inputs"));
    // Pins refuse other inputs.
    let pinned = providers_text(1).replace(" or providers_reported < 2", "").replace(
        "backends = [\"real-api\"]",
        &format!(
            "backends = [\"real-api\"]\npins = {{ scenario = [\"{}\"], workload = [\"{}\"], models = {{ anthropic = \"claude\", openai = \"gpt\" }} }}",
            Digest::of(b"scenario").to_hex(),
            Digest::of(b"workload").to_hex()
        ),
    );
    let p = load(&pinned);
    assert_eq!(
        run(&p, provider_set(&p, "anthropic", &big)).verdict,
        V::Pass
    );
    let mut b = provider_set(&p, "anthropic", &big);
    b[0].manifest.scenario_hash = Digest::of(b"other").to_hex();
    assert!(refused(&p, b).contains("pins"));
    let mut b = provider_set(&p, "anthropic", &big);
    for x in &mut b {
        x.manifest.model = "claude-other".into();
    }
    assert!(refused(&p, b).contains("pins"));
}

/// Cites: HYP-28, HYP-15, HYP-20
#[test]
fn verdict_json_carries_every_field_and_is_never_overwritten() {
    let h = load(BASE);
    let v = run(&h, set(&h, &big));
    let text = v.text();
    assert!(text.ends_with("}\n") && !text[..text.len() - 1].contains('\n'));
    let j = json(&v);
    for k in [
        "verdict_id",
        "verdict",
        "reasons",
        "labels",
        "hypothesis",
        "expected",
        "providers",
        "bundles",
        "ignored_replicates",
        "twin_only_cells",
        "engine_hash",
        "build_hashes",
        "replicates",
        "slices",
    ] {
        assert!(j.get(k).is_some(), "{k}");
    }
    assert_eq!(j["hypothesis"]["id"], "t1");
    assert_eq!(j["hypothesis"]["status"], "candidate");
    assert_eq!(j["expected"]["outcome"], "pass");
    assert_eq!(j["bundles"].as_array().unwrap().len(), 6);
    let s = &j["slices"][0];
    for k in [
        "params",
        "verdict",
        "reasons",
        "labels",
        "cells",
        "controls",
        "decision_cells",
        "values",
        "readings",
    ] {
        assert!(s.get(k).is_some(), "slice {k}");
    }
    let cell = &s["cells"][3];
    assert_eq!(cell["key"], "knob=true,mode=slow");
    assert_eq!(cell["treatment"]["completed"], 4);
    assert!(
        cell["effects"]["cached_token_ratio"]["value"]
            .as_f64()
            .unwrap()
            > 0.29
    );
    assert!(
        cell["effects"]["cached_token_ratio"]["ci_low"].is_number(),
        "CON-18 with its interval"
    );
    assert_eq!(cell["decision"], true);
    assert_eq!(
        s["decision_cells"],
        serde_json::json!(["knob=true,mode=slow"])
    );
    // Keys sorted, no whitespace: serde_json's sorted rendering of the parsed
    // object is the same text.
    assert_eq!(serde_json::to_string(&j).unwrap(), text.trim_end());
    // Written once; never overwritten.
    let runs = tempfile::tempdir().unwrap();
    let path = write(runs.path(), &v).unwrap();
    assert_eq!(std::fs::read_to_string(&path).unwrap(), text);
    assert!(path.ends_with(format!("verdicts/{}/verdict.json", v.verdict_id.to_hex())));
    let e = write(runs.path(), &v).unwrap_err().to_string();
    assert!(e.contains("never overwritten"), "{e}");
}
