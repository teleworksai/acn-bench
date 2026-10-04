//! The percentile bootstrap of HYP-13 and HYP-15: the verdict seed, one named
//! sub-stream per statistic, the ranged-integer sampler, and the percentile
//! indices. Everything here is a pure function of the hypothesis hash and the
//! per-replicate values, so the same inputs give the same bounds on every run.

use acn_trace::identity::{self, Digest, IdentityError, Preimage};
use rand_chacha::ChaCha20Rng;
use rand_core::Rng as _;

/// HYP-15: the number of resamples of every bootstrap.
pub const B: usize = 10_000;

/// The verdict seed: the derived seed (CON-30) of
/// `blake3("acn-bench/verdict/v1\0" ‖ hypothesis_hash)` (HYP-15). It depends on
/// the hypothesis alone, so no choice of bundles moves the resampling noise.
pub fn verdict_seed(hypothesis_hash: &Digest) -> Result<u64, IdentityError> {
    Ok(identity::derived_seed(
        &Preimage::new("acn-bench/verdict/v1")?
            .digest(hypothesis_hash)
            .finish(),
    ))
}

/// The function part of a sub-stream name: `effect` serves `effect`,
/// `rel_effect`, `ci_low` and `ci_high` alike (HYP-15).
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum Function {
    Effect,
    NoiseFloor,
}

impl Function {
    #[must_use]
    pub fn as_str(self) -> &'static str {
        match self {
            Self::Effect => "effect",
            Self::NoiseFloor => "noise_floor",
        }
    }
}

/// `hyp.bootstrap/<slice key>/<cell key>/<quantity>/<function>` (HYP-15).
#[must_use]
pub fn stream_name(slice_key: &str, cell_key: &str, quantity: &str, function: Function) -> String {
    format!(
        "hyp.bootstrap/{slice_key}/{cell_key}/{quantity}/{}",
        function.as_str()
    )
}

/// A uniform integer in `0..n`, `n > 0`: Lemire's multiply-shift on one 64-bit
/// draw, with rejection of the biased low products (ADR-19). CON-5(a) pins its
/// output by golden vector.
pub fn below(rng: &mut ChaCha20Rng, n: u64) -> u64 {
    debug_assert!(n > 0);
    let threshold = n.wrapping_neg() % n;
    loop {
        let m = u128::from(rng.next_u64()) * u128::from(n);
        #[allow(clippy::cast_possible_truncation)] // the low 64 bits, by design
        let low = m as u64;
        if low >= threshold {
            #[allow(clippy::cast_possible_truncation)] // m < 2^64 · n, so m >> 64 < n
            return (m >> 64) as u64;
        }
    }
}

/// The indices of the lower and upper bounds among `b` sorted statistics at
/// confidence `ci`: `k = round_half_even(b × (1 − ci) / 2)`, evaluated left to
/// right in double precision, and `b − 1 − k` (HYP-15).
#[must_use]
pub fn percentile_indices(b: usize, ci: f64) -> (usize, usize) {
    #[allow(clippy::cast_precision_loss)] // b is 10 000
    let k = (b as f64 * (1.0 - ci) / 2.0).round_ties_even();
    #[allow(clippy::cast_possible_truncation, clippy::cast_sign_loss)] // 0 ≤ k ≤ b/2
    let k = k as usize;
    (k, b.saturating_sub(1).saturating_sub(k))
}

/// The bounds at `ci` of a sorted set of resampled statistics.
#[must_use]
pub fn bounds(sorted: &[f64], ci: f64) -> Option<(f64, f64)> {
    let (lo, hi) = percentile_indices(sorted.len(), ci);
    Some((*sorted.get(lo)?, *sorted.get(hi)?))
}

fn mean_of(values: &[f64], idx: &[usize]) -> f64 {
    let mut sum = 0.0;
    for i in idx {
        sum += values[*i];
    }
    #[allow(clippy::cast_precision_loss)]
    let n = idx.len() as f64;
    sum / n
}

fn draw(rng: &mut ChaCha20Rng, n: usize, out: &mut Vec<usize>) {
    out.clear();
    for _ in 0..n {
        #[allow(clippy::cast_possible_truncation)] // below(n) < n, a usize
        out.push(below(rng, n as u64) as usize);
    }
}

/// The `B` sorted statistics of `effect`: treatment and control resampled as
/// pairs by replicate index, each resample the mean of the drawn treatment values
/// minus the mean of the drawn control values (HYP-15). `None` when the arms
/// differ in length or are empty.
#[must_use]
pub fn effect_stats(treatment: &[f64], control: &[f64], mut rng: ChaCha20Rng) -> Option<Vec<f64>> {
    let n = treatment.len();
    if n == 0 || control.len() != n {
        return None;
    }
    let mut idx = Vec::with_capacity(n);
    let mut stats = Vec::with_capacity(B);
    for _ in 0..B {
        draw(&mut rng, n, &mut idx);
        stats.push(mean_of(treatment, &idx) - mean_of(control, &idx));
    }
    sort(stats)
}

/// The `B` sorted statistics behind `noise_floor` for one control arm: the
/// replicates with even and odd index are resampled independently, `n/2` draws
/// each, the even half first, and each resample is `mean(even) − mean(odd)`
/// (HYP-13). `None` when `values` is empty or of odd length.
#[must_use]
pub fn split_half_stats(values: &[f64], mut rng: ChaCha20Rng) -> Option<Vec<f64>> {
    if values.is_empty() || values.len() % 2 != 0 {
        return None;
    }
    let even: Vec<f64> = values.iter().step_by(2).copied().collect();
    let odd: Vec<f64> = values.iter().skip(1).step_by(2).copied().collect();
    let h = even.len();
    let (mut ie, mut io) = (Vec::with_capacity(h), Vec::with_capacity(h));
    let mut stats = Vec::with_capacity(B);
    for _ in 0..B {
        draw(&mut rng, h, &mut ie);
        draw(&mut rng, h, &mut io);
        stats.push(mean_of(&even, &ie) - mean_of(&odd, &io));
    }
    sort(stats)
}

/// Half the width of the interval at `ci` of split-half statistics.
#[must_use]
pub fn half_width(sorted: &[f64], ci: f64) -> Option<f64> {
    bounds(sorted, ci).map(|(lo, hi)| (hi - lo) / 2.0)
}

fn sort(mut stats: Vec<f64>) -> Option<Vec<f64>> {
    if stats.iter().any(|v| !v.is_finite()) {
        return None;
    }
    stats.sort_by(f64::total_cmp);
    Some(stats)
}

/// The generator of the sub-stream `name` under the verdict seed.
pub fn stream(verdict_seed: u64, name: &str) -> Result<ChaCha20Rng, IdentityError> {
    identity::substream_rng(verdict_seed, name)
}
