//! SPEC 090 in the verdict: a real `sim` bundle split as ATR-11 says, checked
//! against its turn view on every turn (ATR-14), and a bundle whose
//! attribution fails refusing only a verdict that reads it (ATR-15).

#![allow(clippy::unwrap_used, clippy::expect_used, clippy::panic)] // CON-19: tests are exempt

mod common;

use std::collections::BTreeMap;
use std::path::{Path, PathBuf};

use acn_harness::agent::Opts;
use acn_harness::run::{HypothesisArg, RunConfig, run_with_scenario};
use acn_harness::wire::Backend;
use acn_hyp::read::BundleData;
use acn_hyp::verdict::{VerdictError, verdict};
use acn_trace::identity::Mode;
use acn_trace::ingest::LeafKind;
use common::bundles::{Spec, build, bundle, engine, flat};
use common::{BASE, candidate};

/// One way, in nanoseconds.
const D: i64 = 40_000_000;

fn scenario(dir: &Path, delay_us: i64) -> PathBuf {
    let p = dir.join("p.toml");
    let delay = format!("\n[link.delay]\ndelay_us = {delay_us}\njitter_us = 0\n");
    std::fs::write(
        &p,
        format!(
            "schema_version = 1\nname = \"p\"\n\n[[link]]\nname = \"p\"\ndirection = \"up\"\n{delay}\n[[link]]\nname = \"p\"\ndirection = \"down\"\n{delay}"
        ),
    )
    .unwrap();
    p
}

/// A `sim` harness bundle of the smoke workload; its directory.
fn sim(dir: &Path, sc: Option<&Path>) -> PathBuf {
    let workload = Path::new(env!("CARGO_MANIFEST_DIR")).join("../../workloads/harness-smoke.toml");
    let cfg = RunConfig {
        workload,
        backend: Backend::Mockllm,
        model: "mock-auto".into(),
        mode: Mode::Sim,
        arm: "treatment".into(),
        replicates: 2,
        vary: BTreeMap::new(),
        opts: Opts::default(),
        hypothesis: HypothesisArg::None { seed: 11 },
        runs_dir: dir.join("runs"),
        start_dir: dir.to_path_buf(),
        engine_hash: engine(),
        build: build("build"),
        profiles: None,
    };
    run_with_scenario(&cfg, sc).unwrap().dir
}

/// The chat leaves of each turn's critical path, in turn-view order.
fn chat_leaves(dir: &Path) -> Vec<i64> {
    let (_, _, trace) = acn_trace::bundle::verify_views_read_trace(dir).unwrap();
    acn_trace::ingest::critical_paths(&trace)
        .unwrap()
        .iter()
        .map(|p| {
            i64::try_from(p.leaves.iter().filter(|l| l.kind == LeafKind::Chat).count()).unwrap()
        })
        .collect()
}

/// Cites: ATR-1, ATR-10, ATR-11, ATR-14, ATR-22
#[test]
fn a_sim_bundle_over_delay_d_has_2d_of_network_time_per_call() {
    let dir = tempfile::tempdir().unwrap();
    let sc = scenario(dir.path(), D / 1000);
    let b = sim(dir.path(), Some(&sc));
    let data = acn_hyp::read::read(&b).unwrap();
    let ds = data.attrib.as_ref().unwrap();
    let chats = chat_leaves(&b);
    assert_eq!(ds.len(), chats.len());
    assert!(!ds.is_empty());
    for (d, n) in ds.iter().zip(&chats) {
        let p = d.parts;
        assert_eq!(
            p.network_ns + p.model_ns + p.tool_ns + p.retry_ns + p.other_ns,
            p.duration_ns
        );
        // A constant delay, no rate limit and no loss: each call's request and
        // last message take d each, streamed or not (ATR-11).
        assert_eq!(p.network_ns, 2 * D * n, "{d:?}");
        assert_eq!(d.clipped_ns, 0);
        assert_eq!(d.unattributed_link_ns, 0);
    }
    // The verdict's quantity is acn-attrib's share of these turns (ATR-22).
    let parts: Vec<_> = ds.iter().map(|d| d.parts).collect();
    let net: i64 = parts.iter().map(|p| p.network_ns).sum();
    let dur: i64 = parts.iter().map(|p| p.duration_ns).sum();
    #[allow(clippy::cast_precision_loss)]
    let want = net as f64 / dur as f64;
    assert_eq!(
        acn_attrib::core::share(&parts, acn_attrib::core::Cause::Network).unwrap(),
        Some(want)
    );
    assert!(want > 0.0 && want < 1.0, "{want}");

    // No delay, or no scenario: no network time.
    for sc in [Some(scenario(dir.path(), 0)), None] {
        let other = tempfile::tempdir().unwrap();
        let b = sim(other.path(), sc.as_deref());
        let data = acn_hyp::read::read(&b).unwrap();
        for d in data.attrib.as_ref().unwrap() {
            assert_eq!(d.parts.network_ns, 0, "{d:?}");
        }
    }
}

/// The bundles of one slice of `BASE`: two controls and four treatments.
fn set(h: &acn_hyp::Hypothesis) -> Vec<BundleData> {
    let mut out = Vec::new();
    for m in ["fast", "slow"] {
        let s = Spec::new(
            &format!("c-{m}"),
            &[("knob", "false"), ("mode", m)],
            "control",
            flat(4, 0.4),
        );
        out.push(bundle(h, &s));
        for k in ["false", "true"] {
            let s = Spec::new(
                &format!("t-{k}-{m}"),
                &[("knob", k), ("mode", m)],
                "treatment",
                flat(4, 0.4),
            );
            out.push(bundle(h, &s));
        }
    }
    out
}

/// The error `acn-attrib` gives a turn whose leaves overlap (ATR-10).
fn overlap_error() -> String {
    use acn_attrib::core::{Leaf, LeafKind, TurnPath, TurnRow, decompose};
    let leaf = |id: u8, start: i64, end: i64| Leaf {
        span_id: [id; 8],
        kind: LeafKind::Chat,
        start_ns: start,
        end_ns: end,
        placement: None,
    };
    let p = TurnPath {
        session_id: [1; 8],
        turn_id: [9; 8],
        turn_index: 0,
        start_ns: 0,
        end_ns: 100,
        leaves: vec![leaf(1, 0, 60), leaf(2, 50, 100)],
    };
    let t = TurnRow {
        session_id: [1; 8],
        duration_ns: 100,
        model_wait_ns: 110,
        ..TurnRow::default()
    };
    decompose(&[t], &[p], &[]).unwrap_err().to_string()
}

/// Cites: ATR-15
#[test]
fn a_bundle_whose_attribution_failed_refuses_only_a_verdict_that_reads_it() {
    let reads = BASE.replace(
        "secondary = [\"ttft_p50_ms\", \"ttft_p99_ms\"]",
        "secondary = [\"ttft_p50_ms\", \"network_attributable_share\"]",
    );
    assert_ne!(reads, BASE);
    let h = candidate(&reads, "t1").0.unwrap();
    let mut b = set(&h);
    b[0].attrib = Err(overlap_error());
    let dir = b[0].dir.display().to_string();
    match verdict(&h, b, engine()) {
        Err(VerdictError::Refused(m)) => {
            // It names the bundle, the turn and the rule.
            assert!(m.contains(&dir), "{m}");
            assert!(m.contains("turn 0 of session 0101010101010101"), "{m}");
            assert!(m.contains("ATR-15") && m.contains("ATR-10"), "{m}");
        }
        other => panic!("expected a refusal, got {other:?}"),
    }
    // The same failure does not touch a verdict that reads no attribution.
    let h = candidate(BASE, "t1").0.unwrap();
    let mut b = set(&h);
    b[0].attrib = Err(overlap_error());
    assert!(verdict(&h, b, engine()).is_ok());
}

/// Cites: ATR-30
#[test]
fn a_replicate_whose_sums_overflow_refuses_the_verdict() {
    let reads = BASE.replace(
        "secondary = [\"ttft_p50_ms\", \"ttft_p99_ms\"]",
        "secondary = [\"ttft_p50_ms\", \"network_attributable_share\"]",
    );
    let h = candidate(&reads, "t1").0.unwrap();
    let mut b: Vec<BundleData> = set(&h).into_iter().map(|b| with_network(b, 100)).collect();
    // Two turns of one session whose durations sum past i64::MAX.
    if let Ok(ds) = &mut b[0].attrib {
        let mut big = ds[0].clone();
        big.parts.duration_ns = i64::MAX;
        big.parts.model_ns = i64::MAX - big.parts.network_ns;
        ds.push(big.clone());
        ds.push(big);
    }
    match verdict(&h, b, engine()) {
        Err(VerdictError::Refused(m)) => assert!(m.contains("ATR-30"), "{m}"),
        other => panic!("expected a refusal, got {other:?}"),
    }
}

/// Give every session of `b` one turn of 1000 ns, `network` of it on the
/// network, a little more for each replicate so the interval is not a point.
fn with_network(mut b: BundleData, network: i64) -> BundleData {
    let ds = b
        .sessions
        .iter()
        .map(|s| acn_attrib::core::Decomposition {
            session_id: s.session_id,
            turn_index: 0,
            parts: acn_attrib::core::Parts {
                duration_ns: 1000,
                network_ns: network + 10 * s.replicate,
                model_ns: 1000 - network - 10 * s.replicate,
                ..acn_attrib::core::Parts::default()
            },
            hop_ns: BTreeMap::new(),
            queue_wait_ns: None,
            stalls: 0,
            retries: 0,
            clipped_ns: 0,
            unattributed_link_ns: 0,
        })
        .collect();
    b.attrib = Ok(ds);
    b
}

/// Cites: ATR-20, ATR-23
#[test]
fn the_comparison_with_the_control_is_the_verdicts_effect_with_its_interval() {
    // A primary measure: the verdict records each cell's effect of it.
    let reads = BASE.replace(
        "primary = [\"cached_token_ratio\"]",
        "primary = [\"cached_token_ratio\", \"network_attributable_share\"]",
    );
    assert_ne!(reads, BASE);
    let h = candidate(&reads, "t1").0.unwrap();
    // `set` makes, per mode, the control and the knob = false and true
    // treatments: the sixth is knob = true, mode = slow.
    let b: Vec<BundleData> = set(&h)
        .into_iter()
        .enumerate()
        .map(|(k, b)| with_network(b, if k == 5 { 400 } else { 100 }))
        .collect();
    let v = verdict(&h, b, engine()).unwrap();
    let j: serde_json::Value = serde_json::from_str(&v.text()).unwrap();
    let cells = j["slices"][0]["cells"].as_array().unwrap();
    for cell in cells {
        let e = &cell["effects"]["network_attributable_share"];
        let (lo, val, hi) = (
            e["ci_low"].as_f64().unwrap(),
            e["value"].as_f64().unwrap(),
            e["ci_high"].as_f64().unwrap(),
        );
        let want = if cell["key"] == "knob=true,mode=slow" {
            0.3
        } else {
            0.0
        };
        assert!((val - want).abs() < 1e-9, "{cell}");
        assert!(
            lo <= val && val <= hi,
            "the paired bootstrap's interval (HYP-15): {cell}"
        );
    }
}
