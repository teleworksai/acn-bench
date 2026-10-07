//! SPEC 090 §3: the shares and the tail shares, with known answers (ATR-20,
//! ATR-21, ATR-30).

#![allow(clippy::unwrap_used, clippy::expect_used, clippy::panic)] // CON-19: tests are exempt

use acn_attrib::core::{Cause, Parts, share, tail_share};

/// A turn of `duration` with `network` of it on the network, the rest model.
fn turn(duration: i64, network: i64) -> Parts {
    Parts {
        duration_ns: duration,
        network_ns: network,
        model_ns: duration - network,
        ..Parts::default()
    }
}

/// Cites: ATR-20, ATR-30
#[test]
fn a_share_is_a_ratio_of_sums_not_a_mean_of_ratios() {
    let ts = [turn(100, 50), turn(900, 90)];
    // (50 + 90) / (100 + 900), not (0.5 + 0.1) / 2.
    assert_eq!(share(&ts, Cause::Network).unwrap(), Some(0.14));
    assert_eq!(share(&ts, Cause::Model).unwrap(), Some(0.86));
    assert_eq!(share(&ts, Cause::Retry).unwrap(), Some(0.0));
    // Undefined, never zero: no turn, or no time.
    assert_eq!(share(&[], Cause::Network).unwrap(), None);
    assert_eq!(share(&[turn(0, 0)], Cause::Network).unwrap(), None);
    // An overflowing sum is an error, which a verdict refuses: never undefined.
    let e = share(&[turn(i64::MAX, 0), turn(1, 0)], Cause::Network).unwrap_err();
    assert!(e.to_string().contains("ATR-30"), "{e}");
}

/// Cites: ATR-21, ATR-30
#[test]
fn the_tail_is_the_nearest_rank_percentile_with_its_ties() {
    // n = 1: the one turn.
    assert_eq!(
        tail_share(&[turn(10, 4)], Cause::Network, 99).unwrap(),
        Some(0.4)
    );
    // n = 10: rank ceil(9.9) = 10, the longest turn and its ties.
    let mut ts: Vec<Parts> = (1..=8).map(|k| turn(10 * k, 0)).collect();
    ts.push(turn(100, 30));
    ts.push(turn(100, 70));
    assert_eq!(tail_share(&ts, Cause::Network, 99).unwrap(), Some(0.5));
    // n = 100: rank 99, so the two longest turns (the 99th and the 100th).
    let mut ts: Vec<Parts> = (1..=98).map(|k| turn(k, 0)).collect();
    ts.push(turn(1000, 100));
    ts.push(turn(2000, 900));
    assert_eq!(
        tail_share(&ts, Cause::Network, 99).unwrap(),
        Some(1000.0 / 3000.0)
    );
    // Ties at the threshold are all in: 99 turns of 5 and one of 1.
    let mut ts: Vec<Parts> = (0..99).map(|_| turn(5, 1)).collect();
    ts.push(turn(1, 1));
    assert_eq!(tail_share(&ts, Cause::Network, 99).unwrap(), Some(0.2));
    // The order of the turns does not matter.
    let mut rev = ts.clone();
    rev.reverse();
    assert_eq!(
        tail_share(&rev, Cause::Network, 99).unwrap(),
        tail_share(&ts, Cause::Network, 99).unwrap()
    );
    assert_eq!(tail_share(&[], Cause::Network, 99).unwrap(), None);
    assert_eq!(tail_share(&[turn(0, 0)], Cause::Network, 99).unwrap(), None);
    // Other percentiles: p = 1 is rank max(1, ceil(n/100)) = 1, every turn;
    // p = 50 of ten turns is rank 5.
    let ts: Vec<Parts> = (1..=10).map(|k| turn(10 * k, k)).collect();
    assert_eq!(
        tail_share(&ts, Cause::Network, 1).unwrap(),
        share(&ts, Cause::Network).unwrap()
    );
    let top: i64 = (5..=10).sum();
    let dur: i64 = (5..=10).map(|k| 10 * k).sum();
    #[allow(clippy::cast_precision_loss)]
    let want = top as f64 / dur as f64;
    assert_eq!(tail_share(&ts, Cause::Network, 50).unwrap(), Some(want));
    // A percentile outside 1 to 100 is an error, not undefined.
    for p in [0, 101] {
        assert!(tail_share(&ts, Cause::Network, p).is_err(), "{p}");
    }
}
