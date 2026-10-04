//! HYP-20..24, HYP-28 on synthetic bundles: each reason in order, extra
//! replicates ignored, the missing grid cell, Kleene connectives over strict
//! values (a true falsifier with a missing cell fails; a false one does not
//! pass), a zero `noise_floor`, every refusal of HYP-20, config and workload
//! controls, labels including the back-dated freeze and `unpinned-inputs`, the
//! twin rule with decision cells, thresholds, tolerances and effect divergence,
//! per-provider slices, the half-submitted provider and `partial-providers`, and
//! the values of `verdict.json` and how it is written.

#![allow(clippy::unwrap_used, clippy::expect_used, clippy::panic)] // CON-19: tests are exempt

mod common;

use std::collections::BTreeSet;

use acn_hyp::Hypothesis;
use acn_hyp::read::BundleData;
use acn_hyp::verdict::{FORMAT, ProviderStatus, V, Verdict, VerdictError, verdict, write};
use acn_trace::identity::Digest;
use common::bundles::{Spec, alt, build, bundle, engine, extra_replicate, flat};
use common::{BASE, candidate, frozen, frozen_text, with_predicate};
use serde_json::json;

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
    match verdict(h, b, engine()) {
        Err(VerdictError::Refused(m)) => m,
        other => panic!("expected a refusal, got {other:?}"),
    }
}

/// The first slice's reasons.
fn ids(v: &Verdict) -> Vec<&'static str> {
    v.slices[0].reasons.iter().map(|r| r.id.as_str()).collect()
}

/// The file's reasons.
fn file_ids(v: &Verdict) -> Vec<&'static str> {
    v.reasons.iter().map(|r| r.id.as_str()).collect()
}

fn labels(v: &Verdict) -> BTreeSet<&'static str> {
    v.labels.iter().map(|l| l.as_str()).collect()
}

fn provider(v: &Verdict, p: &str) -> ProviderStatus {
    v.providers.as_ref().unwrap()[p]
}

fn json(v: &Verdict) -> serde_json::Value {
    serde_json::from_str(&v.text()).unwrap()
}

fn name_id(name: &str) -> Digest {
    Digest::of(format!("run:{name}").as_bytes())
}

fn drop_named(mut b: Vec<BundleData>, name: &str) -> Vec<BundleData> {
    b.retain(|x| x.run_id != name_id(name));
    b
}

fn find<'a>(b: &'a mut [BundleData], name: &str) -> &'a mut BundleData {
    b.iter_mut().find(|x| x.run_id == name_id(name)).unwrap()
}

/// Cites: HYP-21, HYP-24, HYP-11, HYP-13
#[test]
fn a_slice_passes_fails_or_is_inconclusive_for_each_reason_in_order() {
    let h = load(BASE);
    // p4's shape: refuted when no effect clears the noise floor.
    let v = run(&h, set(&h, &big));
    assert_eq!(v.verdict, V::Pass, "{:?}", v.reasons);
    assert!(v.slices[0].reasons.is_empty() && v.reasons.is_empty());
    assert_eq!(run(&h, set(&h, &none)).verdict, V::Fail);
    // control_missing: the `slow` control was never run.
    let v = run(&h, drop_named(set(&h, &big), "c-slow"));
    assert_eq!(v.verdict, V::Inconclusive);
    assert_eq!(
        ids(&v),
        ["control_missing", "incomplete_cell", "undefined_value"]
    );
    assert_eq!(
        v.slices[0].reasons[0].refers,
        ["knob=false,mode=slow", "knob=true,mode=slow"]
    );
    assert_eq!(file_ids(&v), ["slice_inconclusive", "no_conclusive_slice"]);
    // no_evaluated_cell: controls only.
    let only_controls: Vec<BundleData> = set(&h, &big).into_iter().take(2).collect();
    let v = run(&h, only_controls);
    assert_eq!(
        ids(&v),
        ["no_evaluated_cell", "grid_cell_missing", "undefined_value"]
    );
    // incomplete_cell, then undefined_value, for a treatment arm.
    let mut b = set(&h, &big);
    find(&mut b, "t-true-slow")
        .sessions
        .retain(|s| s.replicate != 2);
    let v = run(&h, b);
    assert_eq!(ids(&v), ["incomplete_cell", "undefined_value"]);
    assert_eq!(v.slices[0].reasons[0].refers, ["knob=true,mode=slow"]);
    assert_eq!(
        json(&v)["slices"][0]["cells"][3]["treatment"]["incomplete"],
        json!([2]),
        "listed, never skipped"
    );
    // ... and for a control arm: every cell that maps to it is incomplete.
    let mut b = set(&h, &big);
    find(&mut b, "c-fast").sessions.retain(|s| s.replicate != 1);
    let v = run(&h, b);
    assert_eq!(ids(&v)[0], "incomplete_cell");
    assert_eq!(
        v.slices[0].reasons[0].refers,
        ["knob=false,mode=fast", "knob=true,mode=fast"]
    );
    // A session that ran no turn leaves its replicate incomplete (ADR-20).
    let mut b = set(&h, &big);
    let t = find(&mut b, "t-false-fast");
    let sid = t.sessions[0].session_id;
    t.turns.retain(|x| x.session_id != sid);
    let v = run(&h, b);
    assert_eq!(ids(&v)[0], "incomplete_cell");
    // A replicate whose own value is undefined is listed too.
    let mut b = set(&h, &big);
    find(&mut b, "c-fast").calls[1].input_tokens = None;
    let v = run(&h, b);
    assert_eq!(v.verdict, V::Inconclusive);
    assert_eq!(
        json(&v)["slices"][0]["controls"][0]["arm"]["undefined"]["cached_token_ratio"],
        json!([1])
    );
    // grid_cell_missing: a cell with no runs; with the aggregate it is undefined.
    let v = run(&h, drop_named(set(&h, &big), "t-true-fast"));
    assert_eq!(ids(&v), ["grid_cell_missing", "undefined_value"]);
    assert_eq!(v.slices[0].reasons[0].refers, ["knob=true,mode=fast"]);
    // A zero noise floor: no variation, nothing to compare with.
    let mut b = set(&h, &big);
    for x in &mut b {
        let t = x.manifest.params["arms"] == "treatment";
        for c in &mut x.calls {
            c.cache_read_tokens = Some(40_000 + if t { 30_000 } else { 0 });
        }
    }
    let v = run(&h, b);
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
    assert_eq!(
        v.ignored[&name_id("t-true-slow").to_hex()],
        BTreeSet::from([4])
    );
    assert_eq!(v.ignored[&name_id("c-fast").to_hex()], BTreeSet::from([9]));
    assert_eq!(json(&v)["ignored_replicates"].as_array().unwrap().len(), 2);
    // The same evidence: the slices are identical, only the bundle set differs.
    assert_eq!(json(&v)["slices"], json(&plain)["slices"]);
    assert_ne!(v.verdict_id, plain.verdict_id);
}

/// Cites: HYP-20, HYP-6, HYP-9
#[test]
fn every_refusal_of_hyp_20() {
    let h = load(BASE);
    let with = |f: &dyn Fn(Spec) -> Spec| slice_set(&h, 4, &[], "", f, &big);
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
    let e = verdict(&h, set(&h, &big), Digest::of(b"other engine")).unwrap_err();
    assert!(e.to_string().contains("engine_hash"), "{e}");
    let b = with(&|mut s| {
        if s.name == "t-true-fast" {
            s.build = "other".into();
        }
        s
    });
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
        run(&h, b.clone()).verdict,
        V::Pass,
        "one cell's replicates from two bundles"
    );
    // ... but the two bundles of one arm share a scenario.
    find(&mut b, "rest").manifest.scenario_hash = Digest::of(b"other").to_hex();
    assert!(refused(&h, b).contains("bundles of slice"));
    // The same run given twice.
    let mut b = set(&h, &big);
    b.push(b[0].clone());
    assert!(refused(&h, b).contains("given twice"));
    // Mock and real backends.
    let b = with(&|s| {
        if s.name == "c-fast" {
            s.backend("openai", "gpt")
        } else {
            s
        }
    });
    assert!(refused(&h, b).contains("mixes mockllm and real"));
    // A treatment and its control on different scenarios, or workloads.
    let b = with(&|mut s| {
        if s.name == "c-slow" {
            s.scenario = Digest::of(b"other").to_hex();
        }
        s
    });
    assert!(refused(&h, b).contains("and its control differ"));
    let b = with(&|mut s| {
        if s.name == "c-slow" {
            s.workload = Digest::of(b"other").to_hex();
        }
        s
    });
    assert!(refused(&h, b).contains("and its control differ"));
    // One slice, two models.
    let b = with(&|s| {
        if s.name == "t-true-slow" {
            s.backend("mockllm", "mock-auto")
        } else {
            s
        }
    });
    assert!(refused(&h, b).contains("backend or model"));
    // Two new_input_tokens methods.
    let b = with(&|mut s| {
        if s.name == "c-fast" {
            s.method = "bytes_scaled".into();
        }
        s
    });
    assert!(refused(&h, b).contains("new_input_tokens_method"));
    // Arms: exactly one, named, and every session in it.
    let mut b = set(&h, &big);
    b[0].manifest
        .params
        .insert("arms".into(), "control,treatment".into());
    assert!(refused(&h, b).contains("is not one arm"));
    let mut b = set(&h, &big);
    b[0].manifest
        .params
        .insert("arms".into(), "researcher".into());
    assert!(refused(&h, b).contains("is not one arm"));
    let mut b = set(&h, &big);
    b[0].sessions[0].role = "treatment".into();
    assert!(refused(&h, b).contains("is not its arm"));
    // The parameters of HYP-6, and a grid's levels.
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
    let rtt = load(&BASE.replace(
        "mode = { kind = \"enum\", values = [\"fast\", \"slow\"] }",
        "mode = { kind = \"enum\", values = [\"fast\", \"slow\"] }\nrtt = { kind = \"int_range\", min = 0, max = 300, levels = [50, 150] }",
    ));
    let b = vec![bundle(
        &rtt,
        &Spec::new(
            "off",
            &[("knob", "true"), ("mode", "fast"), ("rtt", "100")],
            "treatment",
            flat(4, 0.4),
        ),
    )];
    assert!(refused(&rtt, b).contains("not one of the grid's levels"));
    // netem beside sim; live beside netem.
    let b = with(&|s| {
        if s.name == "c-fast" {
            s.mode("netem")
        } else {
            s
        }
    });
    assert!(refused(&h, b).contains("netem bundles beside sim"));
    let b = with(&|s| {
        if s.arm == "control" {
            s.mode("netem")
        } else {
            s.mode("live")
        }
    });
    assert!(refused(&h, b).contains("live and netem"));
    assert!(refused(&h, Vec::new()).contains("no bundles"));
}

fn workload_text(inherits: &str) -> String {
    BASE.replace(
        "config = { knob = false }",
        &format!("workload = \"plain_rpc\"{inherits}"),
    )
}

/// Cites: HYP-8, HYP-20
#[test]
fn a_workload_control_is_keyed_by_what_it_inherits_and_never_doubled() {
    let h = load(&workload_text("\ninherits = [\"mode\"]"));
    let v = run(&h, set(&h, &big));
    assert_eq!(v.verdict, V::Pass, "{:?}", v.slices[0].reasons);
    let j = json(&v);
    let controls = j["slices"][0]["controls"].as_array().unwrap();
    assert_eq!(controls.len(), 2);
    assert!(controls.iter().all(|c| c["kind"] == "workload"));
    assert_eq!(controls[0]["key"], "mode=fast");
    let cells = j["slices"][0]["cells"].as_array().unwrap();
    assert_eq!(
        cells[3]["control"], "mode=slow",
        "a treatment maps by what the control inherits"
    );
    // A second workload-control bundle for one inherited configuration — here
    // differing only in a parameter the control does not inherit — is refused,
    // never merged: it would replace the first one's replicates.
    let mut b = set(&h, &big);
    b.push(bundle(
        &h,
        &Spec::new(
            "c2-fast",
            &[("knob", "true"), ("mode", "fast")],
            "control",
            flat(4, 0.9),
        ),
    ));
    assert!(refused(&h, b).contains("two bundles cover"));
    // A workload control may run another workload, but not another scenario.
    let b = slice_set(
        &h,
        4,
        &[],
        "",
        &|mut s| {
            if s.name == "c-fast" {
                s.workload = Digest::of(b"rpc").to_hex();
            }
            s
        },
        &big,
    );
    assert_eq!(run(&h, b).verdict, V::Pass);
    // Without `inherits`, a workload control inherits every range parameter.
    let d = load(
        &workload_text("")
            .replace(
                "mode = { kind = \"enum\", values = [\"fast\", \"slow\"] }",
                "mode = { kind = \"enum\", values = [\"fast\", \"slow\"] }\nrtt = { kind = \"int_range\", min = 0, max = 300, levels = [50, 150] }",
            ),
    );
    let mut b = Vec::new();
    for rtt in ["50", "150"] {
        b.push(bundle(
            &d,
            &Spec::new(
                &format!("c-{rtt}"),
                &[("knob", "false"), ("mode", "fast"), ("rtt", rtt)],
                "control",
                alt(4, 0.40, 0.46),
            ),
        ));
    }
    let v = run(&d, b);
    let keys: Vec<String> = v.slices[0]
        .data
        .controls()
        .iter()
        .map(|c| acn_hyp::slice::key(&c.config))
        .collect();
    assert_eq!(keys, ["rtt=150", "rtt=50"]);
}

fn providers_text(min: Option<u32>, guard: bool) -> String {
    let min = min.map_or(String::new(), |m| {
        format!("\nmin_providers_for_verdict = {m}")
    });
    let text = BASE
        .replace(
            "mode = { kind = \"enum\", values = [\"fast\", \"slow\"] }",
            "mode = { kind = \"enum\", values = [\"fast\", \"slow\"] }\nprovider = { kind = \"enum\", values = [\"anthropic\", \"openai\"] }",
        )
        .replace(
            "twin_required = false",
            &format!("twin_required = false{min}\nbackends = [\"real-api\"]"),
        );
    if guard {
        text.replace(
            "[expected]",
            "inconclusive_if = \"replicates < 4 or providers_reported < 2\"\n\n[expected]",
        )
    } else {
        text
    }
}

fn provider_set(
    h: &Hypothesis,
    p: &str,
    extra: &[(&str, &str)],
    effect: &dyn Fn(bool, &str) -> f64,
) -> Vec<BundleData> {
    let model = if p == "openai" { "gpt" } else { "claude" };
    let mut vary = vec![("provider", p)];
    vary.extend_from_slice(extra);
    let tag: String = vary.iter().map(|(_, v)| format!("{v}-")).collect();
    slice_set(h, 4, &vary, &tag, &|s| s.backend(p, model), effect)
}

/// Cites: HYP-20, HYP-24, HYP-23, HYP-12
#[test]
fn providers_are_judged_side_by_side_and_a_missing_one_is_shown() {
    let h = load(&providers_text(Some(2), true));
    let mut b = provider_set(&h, "anthropic", &[], &big);
    b.extend(provider_set(&h, "openai", &[], &none));
    let v = run(&h, b.clone());
    assert_eq!(v.slices.len(), 2);
    assert_eq!(v.slices[0].key, "provider=anthropic");
    assert_eq!(provider(&v, "anthropic"), ProviderStatus::Pass);
    assert_eq!(provider(&v, "openai"), ProviderStatus::Fail);
    assert_eq!(v.verdict, V::Fail, "{:?}", v.reasons);
    assert_eq!(v.replicates, Some(4), "the guard's counter");
    assert!(!labels(&v).contains("mock-gated") && labels(&v).contains("exploratory"));
    // vary.provider must be the backend for a real provider.
    let mut wrong = b.clone();
    wrong[0].manifest.backend = "openai".into();
    assert!(refused(&h, wrong).contains("vary.provider"));
    // One provider not run: shown, labelled, and below the minimum.
    let v = run(&h, provider_set(&h, "anthropic", &[], &big));
    assert_eq!(provider(&v, "openai"), ProviderStatus::NotRun);
    assert!(labels(&v).contains("partial-providers"));
    assert_eq!(file_ids(&v), ["guard", "providers_below_minimum"]);
    // The minimum defaults to one: the same set is conclusive, still labelled.
    let h1 = load(&providers_text(None, false));
    let v = run(&h1, provider_set(&h1, "anthropic", &[], &big));
    assert_eq!(v.verdict, V::Pass, "{:?}", v.reasons);
    assert!(labels(&v).contains("partial-providers"));
    // Half-submitted: openai was run and found wanting (its control missing).
    let mut b = provider_set(&h, "anthropic", &[], &big);
    b.extend(drop_named(
        provider_set(&h, "openai", &[], &big),
        "openai-c-fast",
    ));
    let v = run(&h, b);
    assert_eq!(provider(&v, "openai"), ProviderStatus::Inconclusive);
    assert_eq!(v.verdict, V::Inconclusive);
    assert!(
        v.reasons
            .iter()
            .any(|r| r.id.as_str() == "slice_inconclusive" && r.refers == ["provider=openai"])
    );
    // No provider reported: the guard's counter is undefined, and so the guard.
    let v = run(
        &h,
        drop_named(provider_set(&h, "anthropic", &[], &big), "anthropic-c-fast"),
    );
    assert_eq!(v.replicates, None);
    assert_eq!(file_ids(&v)[0], "guard");
}

/// Cites: HYP-24
#[test]
fn a_provider_run_in_some_slices_and_left_out_of_others_is_not_reported() {
    let text = providers_text(Some(1), false).replace(
        "provider = { kind = \"enum\", values = [\"anthropic\", \"openai\"] }",
        "provider = { kind = \"enum\", values = [\"anthropic\", \"openai\"] }\nsize = { kind = \"enum\", values = [\"s\", \"l\"], pooled = false }",
    );
    let h = load(&text);
    let mut b = provider_set(&h, "anthropic", &[("size", "s")], &big);
    b.extend(provider_set(&h, "openai", &[("size", "s")], &big));
    b.extend(provider_set(&h, "openai", &[("size", "l")], &big));
    let v = run(&h, b);
    assert_eq!(v.slices.len(), 4);
    assert_eq!(provider(&v, "anthropic"), ProviderStatus::Inconclusive);
    assert_eq!(provider(&v, "openai"), ProviderStatus::Pass);
    assert_eq!(
        v.verdict,
        V::Inconclusive,
        "anthropic, size = l was left out"
    );
    let r = v
        .reasons
        .iter()
        .find(|r| r.id.as_str() == "slice_inconclusive")
        .unwrap();
    assert_eq!(r.refers, ["provider=anthropic,size=l"]);
    assert_eq!(
        v.replicates,
        Some(4),
        "counted over the reported provider only"
    );
}

/// Cites: HYP-24
#[test]
fn without_a_provider_every_declared_slice_must_be_conclusive() {
    let h = load(&BASE.replace(
        "mode = { kind = \"enum\", values = [\"fast\", \"slow\"] }",
        "mode = { kind = \"enum\", values = [\"fast\", \"slow\"], pooled = false }",
    ));
    let only_fast: Vec<BundleData> = set(&h, &big)
        .into_iter()
        .filter(|b| b.manifest.params["vary.mode"] == "fast")
        .collect();
    let v = run(&h, only_fast);
    assert_eq!(v.slices.len(), 2);
    assert_eq!(v.slices[0].key, "mode=fast");
    assert!(v.providers.is_none() && json(&v)["providers"].is_null());
    assert_eq!(v.verdict, V::Inconclusive);
    assert_eq!(v.reasons[0].refers, ["mode=slow"]);
}

fn twin_text(tol: &str) -> String {
    BASE.replace(
        "twin_required = false",
        &format!("twin_required = true\nsim_live_tolerance = {{ cached_token_ratio = {tol} }}"),
    )
}

/// Live twins of every bundle of `set`: `shift(name)` added to its values, and
/// only the replicate indices `keep` admits.
fn live_of(
    h: &Hypothesis,
    shift: &dyn Fn(&str) -> f64,
    keep: &dyn Fn(&str, usize) -> bool,
) -> Vec<BundleData> {
    let base = alt(4, 0.40, 0.46);
    let ratios = |name: &str, e: f64| {
        base.iter()
            .enumerate()
            .map(|(i, x)| x.filter(|_| keep(name, i)).map(|x| x + e + shift(name)))
            .collect()
    };
    let mut out = Vec::new();
    for m in ["fast", "slow"] {
        let n = format!("c-{m}");
        out.push(bundle(
            h,
            &Spec::new(
                &format!("live-{n}"),
                &[("knob", "false"), ("mode", m)],
                "control",
                ratios(&n, 0.0),
            )
            .mode("live"),
        ));
    }
    for k in [false, true] {
        for m in ["fast", "slow"] {
            let n = format!("t-{k}-{m}");
            out.push(bundle(
                h,
                &Spec::new(
                    &format!("live-{n}"),
                    &[("knob", knob(k)), ("mode", m)],
                    "treatment",
                    ratios(&n, big(k, m)),
                )
                .mode("live"),
            ));
        }
    }
    out
}

fn all(_: &str, _: usize) -> bool {
    true
}

/// Cites: HYP-22, HYP-21, HYP-23
#[test]
fn the_twin_rule_reads_live_bundles_for_divergence_and_twins_the_decision_cells() {
    let h = load(&twin_text("{ abs = 0.03 }"));
    let sim = set(&h, &big);
    // No live bundle: sim-only.
    let v = run(&h, sim.clone());
    assert!(labels(&v).contains("sim-only") && labels(&v).contains("mock-gated"));
    assert!(v.slices[0].twin.is_none());
    // Live twins that agree: every decision cell twinned, no twin label.
    let mut b = sim.clone();
    b.extend(live_of(&h, &|_| 0.01, &all));
    let v = run(&h, b.clone());
    assert_eq!(v.verdict, V::Pass, "{:?}", v.slices[0].reasons);
    assert!(!labels(&v).contains("sim-only") && !labels(&v).contains("partially-twinned"));
    let t = v.slices[0].twin.as_ref().unwrap();
    assert!(t.twinned.values().all(|x| *x) && t.failed.is_empty());
    let d = t.divergences[&3]["cached_token_ratio"];
    assert!((d.treatment.unwrap().unwrap() - 0.01).abs() < 1e-9);
    assert!((d.control.unwrap().unwrap() - 0.01).abs() < 1e-9);
    assert!(d.effect.unwrap().unwrap() < 1e-9);
    let j = json(&v);
    assert_eq!(j["slices"][0]["cells"][3]["twinned"], true);
    assert!(
        (j["slices"][0]["cells"][3]["divergence"]["cached_token_ratio"]["control"]
            .as_f64()
            .unwrap()
            - 0.01)
            .abs()
            < 1e-9
    );
    // A twin outside the tolerance fails the twin rule.
    let mut far = sim.clone();
    far.extend(live_of(
        &h,
        &|n| if n == "t-false-fast" { 0.05 } else { 0.0 },
        &all,
    ));
    let v = run(&h, far);
    assert_eq!(v.verdict, V::Inconclusive);
    assert_eq!(ids(&v)[0], "twin_failed");
    assert_eq!(v.slices[0].reasons[0].refers, ["knob=false,mode=fast"]);
    // Arms within the tolerance while the effect is not: the effect moves by
    // 0.04 (> 0.03) though each arm moves by 0.02.
    let mut flip = sim.clone();
    flip.extend(live_of(
        &h,
        &|n| match n {
            "t-false-fast" => -0.02,
            "c-fast" => 0.02,
            _ => 0.0,
        },
        &all,
    ));
    let v = run(&h, flip);
    assert_eq!(ids(&v)[0], "twin_failed", "{:?}", v.slices[0].reasons);
    // A live cell with no sim cell is listed, and the verdict reads sim only.
    let mut b = drop_named(sim.clone(), "t-true-fast");
    b.extend(live_of(&h, &|_| 0.0, &all));
    let v = run(&h, b);
    assert_eq!(json(&v)["twin_only_cells"], json!(["knob=true,mode=fast"]));
    // Without twin_required, a diverging twin is reported but does not fail.
    let loose = load(&BASE.replace(
        "twin_required = false",
        "twin_required = false\nsim_live_tolerance = { cached_token_ratio = { abs = 0.03 } }",
    ));
    let mut b = set(&loose, &big);
    b.extend(live_of(
        &loose,
        &|n| if n == "t-false-fast" { 0.05 } else { 0.0 },
        &all,
    ));
    let v = run(&loose, b);
    assert_eq!(v.verdict, V::Pass);
    assert!(
        !v.slices[0].twin.as_ref().unwrap().divergences[&0]["cached_token_ratio"]
            .within(acn_hyp::file::Tolerance::Absolute(0.03))
    );
}

/// Cites: HYP-22, HYP-23
#[test]
fn a_decision_cell_is_twinned_with_half_its_replicates_in_both_arms() {
    let h = load(&twin_text("{ abs = 0.03 }"));
    let only =
        |n: usize| move |name: &str, i: usize| (name != "t-true-slow" && name != "c-slow") || i < n;
    // Two of four paired indices in both arms: twinned.
    let mut b = set(&h, &big);
    b.extend(live_of(&h, &|_| 0.0, &only(2)));
    let v = run(&h, b);
    assert!(v.slices[0].twin.as_ref().unwrap().twinned[&3]);
    assert!(!labels(&v).contains("partially-twinned"));
    // One of four: the decision cell is evaluated normally, labelled.
    let mut b = set(&h, &big);
    b.extend(live_of(&h, &|_| 0.0, &only(1)));
    let v = run(&h, b);
    assert_eq!(v.verdict, V::Pass, "{:?}", v.slices[0].reasons);
    assert!(labels(&v).contains("partially-twinned"));
    // Its control's twin missing: not twinned, and its effect divergence undefined.
    let mut b = set(&h, &big);
    b.extend(drop_named(live_of(&h, &|_| 0.0, &all), "live-c-slow"));
    let v = run(&h, b);
    let t = v.slices[0].twin.as_ref().unwrap();
    assert!(!t.twinned[&3] && t.twinned[&0]);
    assert_eq!(t.divergences[&3]["cached_token_ratio"].effect, Some(None));
    assert!(t.failed.contains(&"knob=true,mode=slow".to_owned()));
}

/// Cites: HYP-22
#[test]
fn tolerances_are_relative_or_absolute_and_undefined_divergence_fails() {
    // Relative: |live − sim| / |sim|.
    let h = load(&twin_text("0.1"));
    let mut b = set(&h, &big);
    b.extend(live_of(
        &h,
        &|n| if n == "t-false-fast" { 0.03 } else { 0.0 },
        &all,
    ));
    assert_eq!(run(&h, b).verdict, V::Pass, "0.03 on about 0.43: 7%");
    let mut b = set(&h, &big);
    b.extend(live_of(
        &h,
        &|n| if n == "t-false-fast" { 0.05 } else { 0.0 },
        &all,
    ));
    assert_eq!(
        ids(&run(&h, b))[0],
        "twin_failed",
        "0.05 on about 0.43: 12%"
    );
    // Exactly at an absolute tolerance is within it.
    let e = load(&twin_text("{ abs = 0.25 }").replace("max_over_knobs(abs(effect(cached_token_ratio))) < noise_floor(cached_token_ratio, control)", "max_over_knobs(effect(cached_token_ratio)) > 1"));
    let uniform = |mode: &str, t: f64| {
        let mut out = Vec::new();
        for m in ["fast", "slow"] {
            out.push(bundle(
                &e,
                &Spec::new(
                    &format!("{mode}c-{m}"),
                    &[("knob", "false"), ("mode", m)],
                    "control",
                    flat(4, 0.25),
                )
                .mode(mode),
            ));
        }
        for k in [false, true] {
            for m in ["fast", "slow"] {
                out.push(bundle(
                    &e,
                    &Spec::new(
                        &format!("{mode}t-{k}-{m}"),
                        &[("knob", knob(k)), ("mode", m)],
                        "treatment",
                        flat(4, t),
                    )
                    .mode(mode),
                ));
            }
        }
        out
    };
    let mut b = uniform("sim", 0.25);
    b.extend(uniform("live", 0.5));
    let v = run(&e, b);
    let d = v.slices[0].twin.as_ref().unwrap().divergences[&0]["cached_token_ratio"];
    assert_eq!(d.treatment, Some(Some(0.25)));
    assert!(
        v.slices[0].twin.as_ref().unwrap().failed.is_empty(),
        "0.25 is within 0.25"
    );
    // No index completed in both modes: undefined, which fails.
    let mut b = set(&h, &big);
    find(&mut b, "t-false-fast")
        .sessions
        .retain(|s| s.replicate < 2);
    b.extend(live_of(&h, &|_| 0.0, &|n, i| n != "t-false-fast" || i >= 2));
    let v = run(&h, b);
    assert!(
        v.slices[0]
            .reasons
            .iter()
            .any(|r| r.id.as_str() == "twin_failed" && r.refers == ["knob=false,mode=fast"])
    );
    // A sim arm and its live twin share their scenario.
    let mut b = set(&h, &big);
    let mut live = live_of(&h, &|_| 0.0, &all);
    for x in &mut live {
        x.manifest.scenario_hash = Digest::of(b"s2").to_hex();
    }
    b.extend(live);
    assert!(refused(&h, b).contains("live twin"));
}

/// Cites: HYP-21, HYP-20
#[test]
fn live_only_and_netem_only_sets_are_judged_on_their_own_and_builds_may_differ_by_mode() {
    let h = load(BASE);
    let live = slice_set(&h, 4, &[], "", &|s| s.mode("live"), &big);
    let v = run(&h, live);
    assert_eq!(v.verdict, V::Pass);
    assert!(!labels(&v).contains("sim-only") && v.slices[0].twin.is_none());
    let netem = slice_set(&h, 4, &[], "", &|s| s.mode("netem"), &big);
    assert_eq!(run(&h, netem).verdict, V::Pass);
    let tw = load(&twin_text("{ abs = 0.03 }"));
    let mut b = set(&tw, &big);
    let mut live = live_of(&tw, &|_| 0.0, &all);
    for x in &mut live {
        x.manifest.build = build("other");
    }
    b.extend(live);
    let v = run(&tw, b);
    assert_eq!(v.build_hashes.len(), 2, "one build per mode is allowed");
}

/// Cites: HYP-23, HYP-9
#[test]
fn labels_follow_the_file_and_the_bundles_and_a_frozen_file_needs_its_seed() {
    let text = frozen_text(BASE);
    let (h, _root) = frozen(&text, "t1");
    let h = h.unwrap();
    let full = |h: &Hypothesis, with: &dyn Fn(Spec) -> Spec| slice_set(h, 20, &[], "", with, &big);
    let v = run(&h, full(&h, &|s| s));
    assert_eq!(labels(&v), BTreeSet::from(["mock-gated", "sim-only"]));
    // A back-dated freeze: bundles run while the file was a candidate.
    let v = run(
        &h,
        full(&h, &|mut s| {
            s.status = Some("candidate".into());
            s
        }),
    );
    assert!(labels(&v).contains("exploratory"));
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
    assert!(labels(&run(&u, full(&u, &|s| s))).contains("unpinned-inputs"));
    // Not for a candidate with real-api, nor a pinned file.
    let c = load(&providers_text(Some(1), false));
    assert!(
        !labels(&run(&c, provider_set(&c, "anthropic", &[], &big))).contains("unpinned-inputs")
    );
    // Pins refuse other inputs.
    let pinned = providers_text(Some(1), false).replace(
        "backends = [\"real-api\"]",
        &format!(
            "backends = [\"real-api\"]\npins = {{ scenario = [\"{}\"], workload = [\"{}\"], models = {{ anthropic = \"claude\", openai = \"gpt\" }} }}",
            Digest::of(b"scenario").to_hex(),
            Digest::of(b"workload").to_hex()
        ),
    );
    let p = load(&pinned);
    let v = run(&p, provider_set(&p, "anthropic", &[], &big));
    assert_eq!(v.verdict, V::Pass);
    assert!(!labels(&v).contains("unpinned-inputs"));
    for change in [0, 1, 2] {
        let mut b = provider_set(&p, "anthropic", &[], &big);
        for x in &mut b {
            match change {
                0 => x.manifest.scenario_hash = Digest::of(b"other").to_hex(),
                1 => x.manifest.workload_hash = Digest::of(b"other").to_hex(),
                _ => x.manifest.model = "claude-other".into(),
            }
        }
        assert!(refused(&p, b).contains("pins"), "change {change}");
    }
}

/// Cites: HYP-28, HYP-15, HYP-20, HYP-4
#[test]
fn verdict_json_carries_every_field_and_is_written_once_under_runs() {
    let h = load(BASE);
    let v = run(&h, set(&h, &big));
    let text = v.text();
    assert!(text.ends_with("}\n") && !text[..text.len() - 1].contains('\n'));
    let j = json(&v);
    assert_eq!(j["format"], FORMAT);
    assert_eq!(j["verdict_id"], v.verdict_id.to_hex());
    assert_eq!(j["verdict"], "pass");
    assert_eq!(j["reasons"], json!([]));
    assert_eq!(
        j["labels"],
        json!(["exploratory", "mock-gated", "sim-only"])
    );
    assert_eq!(
        j["hypothesis"],
        json!({"id": "t1", "status": "candidate", "hash": h.hash().to_hex()})
    );
    assert_eq!(j["expected"], json!({"outcome": "pass", "note": null}));
    assert!(j["providers"].is_null());
    assert_eq!(j["bundles"].as_array().unwrap().len(), 6);
    assert_eq!(j["bundles"][0]["bundle_digest"], v.bundles[0].1.to_hex());
    assert_eq!(j["bundles"][0]["mode"], "sim");
    assert_eq!(j["build_hashes"], json!([build("build").build_hash]));
    assert_eq!(j["engine_hash"], engine().to_hex());
    assert_eq!(j["replicates"], 4);
    assert_eq!(j["ignored_replicates"], json!([]));
    assert_eq!(j["twin_only_cells"], json!([]));
    let s = &j["slices"][0];
    assert_eq!(s["verdict"], "pass");
    assert_eq!(s["outcome"], "not_refuted");
    assert_eq!(s["falsifier"], false);
    assert_eq!(s["replicates"], 4);
    let cell = &s["cells"][3];
    assert_eq!(cell["key"], "knob=true,mode=slow");
    assert_eq!(cell["params"], json!({"knob": "true", "mode": "slow"}));
    assert_eq!(cell["control"], "knob=false,mode=slow");
    assert_eq!(cell["treatment"]["completed"], 4);
    assert_eq!(cell["treatment"]["incomplete"], json!([]));
    let e = &cell["effects"]["cached_token_ratio"];
    let (lo, val, hi) = (
        e["ci_low"].as_f64().unwrap(),
        e["value"].as_f64().unwrap(),
        e["ci_high"].as_f64().unwrap(),
    );
    assert!(
        lo <= val && val <= hi && (val - 0.3).abs() < 1e-9,
        "CON-18 with its interval"
    );
    assert_eq!(cell["decision"], true);
    assert!(
        cell["twinned"].is_null() && cell["divergence"].is_null(),
        "no twin"
    );
    assert_eq!(s["decision_cells"], json!(["knob=true,mode=slow"]));
    assert_eq!(s["controls"][0]["kind"], "config");
    let reading = s["readings"]
        .as_array()
        .unwrap()
        .iter()
        .find(|r| r["term"].as_str().unwrap().contains(":treatment"))
        .unwrap();
    assert_eq!(reading["completed"], 4);
    assert!(
        s["values"]
            .as_array()
            .unwrap()
            .iter()
            .any(|x| x["value"] == false)
    );
    // Written once, under runs/ only, never overwritten.
    let tmp = tempfile::tempdir().unwrap();
    let e = write(tmp.path(), &v).unwrap_err();
    assert!(
        matches!(e, VerdictError::Refused(ref m) if m.contains("HYP-4")),
        "{e}"
    );
    let runs = tmp.path().join("runs");
    // A directory left by an interrupted write does not block the verdict.
    std::fs::create_dir_all(runs.join("verdicts").join(v.verdict_id.to_hex())).unwrap();
    let path = write(&runs, &v).unwrap();
    assert_eq!(std::fs::read_to_string(&path).unwrap(), text);
    assert!(path.ends_with(format!("verdicts/{}/verdict.json", v.verdict_id.to_hex())));
    assert!(matches!(write(&runs, &v), Err(VerdictError::Exists(_))));
    assert_eq!(
        std::fs::read_dir(path.parent().unwrap()).unwrap().count(),
        1,
        "no temporary file left"
    );
}

/// Cites: HYP-15
#[test]
fn canonical_json_escapes_strings_and_nulls_what_is_undefined() {
    use acn_hyp::json::J;
    let j = J::obj([
        ("a\"b", J::str("x\\y\n\t\u{1}\u{8}\u{c}é")),
        ("f", J::Float(f64::NAN)),
        ("g", J::Float(150.0)),
        ("i", J::count(3)),
        ("n", J::num(None)),
    ]);
    assert_eq!(
        j.render(),
        "{\"a\\\"b\":\"x\\\\y\\n\\t\\u0001\\b\\fé\",\"f\":null,\"g\":150.0,\"i\":3,\"n\":null}\n"
    );
    let back: serde_json::Value = serde_json::from_str(&j.render()).unwrap();
    assert_eq!(back["a\"b"], "x\\y\n\t\u{1}\u{8}\u{c}é");
    // The same escaping as acn-trace's canonical manifest, which is serde_json's.
    assert_eq!(
        serde_json::to_string("x\\y\n\t\u{1}\u{8}\u{c}é").unwrap(),
        "\"x\\\\y\\n\\t\\u0001\\b\\fé\""
    );
}
