//! HYP-15: the verdict seed and its known answer, the sub-stream names, the
//! ranged-integer sampler's golden vector, the percentile indices (at the default
//! level and at 0.9), the `noise_floor` halves, and bootstraps that repeat bit
//! for bit; and at the verdict level, `verdict_id`'s known answer, a
//! byte-identical `verdict.json` across evaluations and argument orders, and a
//! verdict seed that ignored replicates and twin bundles do not move.

#![allow(clippy::unwrap_used, clippy::expect_used, clippy::panic)] // CON-19: tests are exempt

mod common;

use acn_hyp::bootstrap::{
    B, Function, below, bounds, effect_stats, half_width, percentile_indices, split_half_stats,
    stream, stream_name, verdict_seed,
};
use std::num::NonZeroU64;

use acn_trace::identity::{self, Digest};
use rand_core::Rng as _;

/// Cites: HYP-15, CON-27, CON-30
#[test]
fn the_verdict_seed_matches_its_known_answer_and_depends_on_the_hypothesis_alone() {
    let h = Digest(*blake3::hash(b"a hypothesis file").as_bytes());
    // Computed with the blake3 crate directly, not through `Preimage`: the context
    // bare and zero-terminated, then the raw 32-byte hash.
    let mut pre = b"acn-bench/verdict/v1\0".to_vec();
    pre.extend_from_slice(&h.0);
    let d = blake3::hash(&pre);
    let mut first = [0u8; 8];
    first.copy_from_slice(&d.as_bytes()[..8]);
    let expected = u64::from_le_bytes(first) & !(1u64 << 63);
    assert_eq!(verdict_seed(&h).unwrap(), expected);
    assert_eq!(verdict_seed(&h).unwrap(), 335_174_749_808_925_407);
    assert_ne!(verdict_seed(&Digest::ZERO).unwrap(), expected);
}

/// Cites: HYP-15, CON-30
#[test]
fn each_statistic_draws_from_its_own_named_sub_stream() {
    assert_eq!(
        stream_name(
            "provider=openai",
            "knob=true,mode=fast",
            "cost_per_success",
            Function::Effect
        ),
        "hyp.bootstrap/provider=openai/knob=true,mode=fast/cost_per_success/effect"
    );
    assert_eq!(
        stream_name("", "knob=false", "q", Function::NoiseFloor),
        "hyp.bootstrap//knob=false/q/noise_floor",
        "the slice key is empty for a file with one slice"
    );
    let name = stream_name("", "knob=false", "q", Function::Effect);
    let mut a = stream(7, &name).unwrap();
    let mut b = identity::substream_rng(7, &name).unwrap();
    assert_eq!(
        a.next_u64(),
        b.next_u64(),
        "CON-30(b) under the verdict seed"
    );
}

/// Cites: HYP-15, CON-5
#[test]
fn the_ranged_integer_sampler_matches_its_golden_vector() {
    // The generator's first raw outputs are pinned by acn-trace's CON-5(a) test:
    // 1931434202297228593, 1639166351897295282, 10337616115050100999. For n
    // these map to floor(x · n / 2^64), none falling below the rejection threshold
    // (2^64 mod n); computed independently of this crate.
    let s = identity::replicate_seed(42, 3).unwrap();
    for (n, expected) in [(20u64, [2u64, 1, 11]), (10, [1, 0, 5]), (7, [0, 0, 3])] {
        let mut rng = identity::substream_rng(s, "trace.ids").unwrap();
        let nz = NonZeroU64::new(n).unwrap();
        let got = [
            below(&mut rng, nz),
            below(&mut rng, nz),
            below(&mut rng, nz),
        ];
        assert_eq!(got, expected, "n = {n}");
    }
    // Rejection: with n = 2^63 + 1 half the low products are rejected. A reference
    // written with u128 remainders (not the wrapping form) must agree.
    let n = (1u64 << 63) + 1;
    let mut rng = identity::substream_rng(s, "trace.ids").unwrap();
    let mut raw = rng.clone();
    let threshold = u64::try_from((1u128 << 64) % u128::from(n)).unwrap();
    for _ in 0..64 {
        let got = below(&mut rng, NonZeroU64::new(n).unwrap());
        let expected = loop {
            let m = u128::from(raw.next_u64()) * u128::from(n);
            if (m % (1u128 << 64)) >= u128::from(threshold) {
                break u64::try_from(m >> 64).unwrap();
            }
        };
        assert_eq!(got, expected);
        assert!(got < n);
    }
}

/// Cites: HYP-15
#[test]
fn the_percentile_indices_round_half_even_in_double_precision() {
    assert_eq!(percentile_indices(B, 0.95), (250, 9749));
    // 10000 × (1 − 0.9) / 2 is 499.99999999999994 in double precision: it rounds
    // to 500; truncation would give 499.
    assert!(10_000.0 * (1.0 - std::hint::black_box(0.9)) / 2.0 < 500.0);
    assert_eq!(percentile_indices(B, 0.9), (500, 9499));
    assert_eq!(percentile_indices(B, 0.99), (50, 9949));
    // A tie: 4 × 0.25 / 2 = 0.5 rounds to the even 0, where `round` gives 1.
    assert_eq!(percentile_indices(4, 0.75), (0, 3));
    // A level so small that the indices cross has no bounds (load refuses it).
    assert_eq!(percentile_indices(B, 0.0001), (5000, 4999));
    let sorted_small: Vec<f64> = (0..B).map(|i| i as f64).collect();
    assert_eq!(bounds(&sorted_small, 0.0001), None);
    let sorted: Vec<f64> = (0..B).map(|i| i as f64).collect();
    assert_eq!(bounds(&sorted, 0.95), Some((250.0, 9749.0)));
    assert_eq!(half_width(&sorted, 0.95), Some((9749.0 - 250.0) / 2.0));
}

/// Cites: HYP-15, HYP-13
#[test]
fn bootstraps_repeat_bit_for_bit_and_resample_pairs_and_halves() {
    let t = [0.6, 0.9, 0.7, 0.8];
    let c = [0.5, 0.5, 0.5, 0.5];
    let rng = || stream(11, "hyp.bootstrap//x=1/q/effect").unwrap();
    let a = effect_stats(&t, &c, rng()).unwrap();
    assert_eq!(a.len(), B);
    assert_eq!(a, effect_stats(&t, &c, rng()).unwrap());
    assert!(a.windows(2).all(|w| w[0] <= w[1]), "sorted ascending");
    // Pairs: a constant per-index shift gives the same statistic every time.
    let shifted: Vec<f64> = c.iter().map(|x| x + 0.25).collect();
    let s = effect_stats(&shifted, &c, rng()).unwrap();
    assert!(s.iter().all(|v| (v - 0.25).abs() < 1e-12));
    assert!(effect_stats(&t, &c[..3], rng()).is_none(), "unpaired");
    // Halves: even indices 0, 2 and odd indices 1, 3. Even all 1.0 and odd all
    // 0.0 makes every resample exactly 1.0, whatever is drawn.
    let floor = split_half_stats(&[1.0, 0.0, 1.0, 0.0], rng()).unwrap();
    assert!(floor.iter().all(|v| *v == 1.0));
    assert_eq!(half_width(&floor, 0.95), Some(0.0));
    let swapped = split_half_stats(&[0.0, 1.0, 0.0, 1.0], rng()).unwrap();
    assert!(swapped.iter().all(|v| *v == -1.0), "even minus odd");
    assert!(
        split_half_stats(&[1.0, 2.0, 3.0], rng()).is_none(),
        "odd count"
    );
    // The draws: per resample, n/2 indices for the even half, then n/2 for the
    // odd half, from one stream; a reference loop must agree element for element.
    let v: Vec<f64> = (0..8).map(|i| ((i * 37) % 11) as f64).collect();
    let (even, odd): (Vec<f64>, Vec<f64>) = (
        v.iter().step_by(2).copied().collect(),
        v.iter().skip(1).step_by(2).copied().collect(),
    );
    let mut r = rng();
    let four = NonZeroU64::new(4).unwrap();
    let mut expected: Vec<f64> = (0..B)
        .map(|_| {
            let e: f64 = (0..4)
                .map(|_| even[below(&mut r, four) as usize])
                .sum::<f64>()
                / 4.0;
            let o: f64 = (0..4)
                .map(|_| odd[below(&mut r, four) as usize])
                .sum::<f64>()
                / 4.0;
            e - o
        })
        .collect();
    expected.sort_by(f64::total_cmp);
    assert_eq!(split_half_stats(&v, rng()).unwrap(), expected);
    // The same for pairs: one draw of n indices serves both arms.
    let t: Vec<f64> = (0..4).map(|i| f64::from(i * 3 % 5)).collect();
    let c: Vec<f64> = (0..4).map(|i| f64::from(i * 2 % 3)).collect();
    let mut r = rng();
    let mut expected: Vec<f64> = (0..B)
        .map(|_| {
            let idx: Vec<usize> = (0..4).map(|_| below(&mut r, four) as usize).collect();
            idx.iter().map(|i| t[*i]).sum::<f64>() / 4.0
                - idx.iter().map(|i| c[*i]).sum::<f64>() / 4.0
        })
        .collect();
    expected.sort_by(f64::total_cmp);
    assert_eq!(effect_stats(&t, &c, rng()).unwrap(), expected);
    // A statistic that overflows is not finite: no bounds.
    assert!(effect_stats(&[f64::MAX; 2], &[-f64::MAX; 2], rng()).is_none());
    assert_eq!(Function::NoiseFloor.as_str(), "noise_floor");
}

/// Cites: HYP-15, CON-27
#[test]
fn verdict_id_matches_its_known_answer() {
    let h = Digest(*blake3::hash(b"a hypothesis file").as_bytes());
    let pairs = [
        (Digest::of(b"run a"), Digest::of(b"digest a")),
        (Digest::of(b"run b"), Digest::of(b"digest b")),
    ];
    // The blake3 crate directly: the bare context, its zero byte, the raw
    // hypothesis hash, then each run_id and bundle_digest, raw, in order.
    let mut pre = b"acn-bench/verdict_id/v1\0".to_vec();
    pre.extend_from_slice(&h.0);
    for (r, d) in &pairs {
        pre.extend_from_slice(&r.0);
        pre.extend_from_slice(&d.0);
    }
    let expected = Digest(*blake3::hash(&pre).as_bytes());
    assert_eq!(acn_hyp::verdict::verdict_id(&h, &pairs).unwrap(), expected);
    assert_eq!(
        expected.to_hex(),
        acn_hyp::verdict::verdict_id(&h, &pairs).unwrap().to_hex()
    );
    assert_ne!(
        acn_hyp::verdict::verdict_id(&h, &pairs[..1]).unwrap(),
        expected
    );
}

fn sample_set(h: &acn_hyp::Hypothesis) -> Vec<acn_hyp::read::BundleData> {
    use common::bundles::{Spec, alt, bundle};
    let base = alt(4, 0.40, 0.46);
    let mut out = Vec::new();
    for m in ["fast", "slow"] {
        out.push(bundle(
            h,
            &Spec::new(
                &format!("c-{m}"),
                &[("knob", "false"), ("mode", m)],
                "control",
                base.clone(),
            ),
        ));
    }
    for k in ["false", "true"] {
        for m in ["fast", "slow"] {
            let e = if k == "true" && m == "slow" {
                0.3
            } else {
                0.05
            };
            let r = base.iter().map(|x| x.map(|x| x + e)).collect();
            out.push(bundle(
                h,
                &Spec::new(
                    &format!("t-{k}-{m}"),
                    &[("knob", k), ("mode", m)],
                    "treatment",
                    r,
                ),
            ));
        }
    }
    out
}

/// Cites: HYP-15
#[test]
fn verdict_json_is_byte_identical_across_evaluations_and_argument_orders() {
    let h = common::candidate(common::BASE, "t1").0.unwrap();
    let a = acn_hyp::verdict::verdict(&h, sample_set(&h), common::bundles::engine()).unwrap();
    let b = acn_hyp::verdict::verdict(&h, sample_set(&h), common::bundles::engine()).unwrap();
    assert_eq!(a.text(), b.text());
    let mut rev = sample_set(&h);
    rev.reverse();
    let r = acn_hyp::verdict::verdict(&h, rev, common::bundles::engine()).unwrap();
    assert_eq!(a.text(), r.text());
    assert_eq!(a.verdict_id, r.verdict_id);
    // Bundles are listed ascending.
    let ids: Vec<String> = a.bundles.iter().map(|(r, _)| r.to_hex()).collect();
    let mut sorted = ids.clone();
    sorted.sort();
    assert_eq!(ids, sorted);
}

/// Cites: HYP-15, HYP-21, HYP-22
#[test]
fn ignored_replicates_and_twin_bundles_do_not_move_the_resampling() {
    use common::bundles::{Spec, alt, bundle, extra_replicate};
    let h = common::candidate(common::BASE, "t1").0.unwrap();
    let plain = acn_hyp::verdict::verdict(&h, sample_set(&h), common::bundles::engine()).unwrap();
    let effects = |v: &acn_hyp::verdict::Verdict| {
        let j: serde_json::Value = serde_json::from_str(&v.text()).unwrap();
        j["slices"][0]["cells"]
            .as_array()
            .unwrap()
            .iter()
            .map(|c| c["effects"].clone())
            .collect::<Vec<_>>()
    };
    let mut more = sample_set(&h);
    extra_replicate(&mut more[2], 6);
    more.push(bundle(
        &h,
        &Spec::new(
            "live",
            &[("knob", "true"), ("mode", "slow")],
            "treatment",
            alt(4, 0.7, 0.76),
        )
        .mode("live"),
    ));
    let v = acn_hyp::verdict::verdict(&h, more, common::bundles::engine()).unwrap();
    assert_ne!(v.verdict_id, plain.verdict_id, "another set, another id");
    assert_eq!(
        effects(&v),
        effects(&plain),
        "the same draws: the seed is the hypothesis's"
    );
}
