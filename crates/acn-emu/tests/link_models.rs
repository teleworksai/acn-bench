//! Link models (SPEC 020 §2): the statistics of each stage over many messages
//! and several seeds, and the structure the spec fixes (stage order, FIFO,
//! outages, per-stage sub-streams, a golden vector).
#![allow(clippy::unwrap_used, clippy::expect_used, clippy::panic)]

use acn_emu::link::{
    Delay, Direction, DropCause, Fate, Link, LinkModel as _, LinkSpec, Loss, OutageCause,
    OutageMode, Rate, Reorder, Window,
};

const SEEDS: [u64; 5] = [1, 2, 3, 42, 3_173_018_459_045_278_302];
const SECOND: i64 = 1_000_000_000;
const MS: i64 = 1_000_000;

fn spec() -> LinkSpec {
    LinkSpec::new("radio", Direction::Up)
}

/// Send `n` messages of `bytes`, `gap_ns` apart from 0.
fn run(link: &mut Link, n: usize, gap_ns: i64, bytes: u64) -> Vec<Fate> {
    (0..n)
        .map(|i| link.transmit(i as i64 * gap_ns, bytes).unwrap())
        .collect()
}

/// Whether `count` successes of `n` trials are within four standard deviations
/// of probability `p`.
fn within_4_sigma(count: usize, n: usize, p: f64) -> bool {
    let mean = n as f64 * p;
    let sd = (n as f64 * p * (1.0 - p)).sqrt();
    (count as f64 - mean).abs() <= 4.0 * sd
}

/// Cites: EMU-5, EMU-9
#[test]
fn independent_loss_matches_its_rate() {
    for seed in SEEDS {
        let mut s = spec();
        s.loss = Some(Loss::Iid { loss_ppm: 50_000 });
        let mut l = Link::new(s, seed).unwrap();
        let n = 200_000;
        let fates = run(&mut l, n, 1, 100);
        let lost = fates
            .iter()
            .filter(|f| f.outcome == Err(DropCause::Loss))
            .count();
        assert!(within_4_sigma(lost, n, 0.05), "seed {seed}: {lost} of {n}");
    }
}

/// Cites: EMU-6, EMU-9
#[test]
fn gilbert_elliott_matches_its_stationary_loss_and_burst_length() {
    let (p_gb, p_bg) = (10_000_u64, 200_000_u64);
    let pi_bad = p_gb as f64 / (p_gb + p_bg) as f64;
    for seed in SEEDS {
        // Lose every message in `bad` and none in `good`: a loss run is a stay
        // in `bad`, geometric with mean 1 / p_bad_good.
        let mut s = spec();
        s.loss = Some(Loss::GilbertElliott {
            p_good_bad_ppm: p_gb,
            p_bad_good_ppm: p_bg,
            loss_good_ppm: 0,
            loss_bad_ppm: 1_000_000,
        });
        let mut l = Link::new(s, seed).unwrap();
        let n = 500_000;
        let lost: Vec<bool> = run(&mut l, n, 1, 100)
            .iter()
            .map(|f| f.outcome.is_err())
            .collect();
        let rate = lost.iter().filter(|x| **x).count() as f64 / n as f64;
        assert!(
            (rate - pi_bad).abs() < 0.1 * pi_bad,
            "seed {seed}: {rate} vs {pi_bad}"
        );
        let mut runs = Vec::new();
        let mut cur = 0;
        for x in &lost {
            if *x {
                cur += 1;
            } else if cur > 0 {
                runs.push(cur);
                cur = 0;
            }
        }
        let mean = runs.iter().sum::<usize>() as f64 / runs.len() as f64;
        let want = 1_000_000.0 / p_bg as f64;
        assert!(
            (mean - want).abs() < 0.05 * want,
            "seed {seed}: burst {mean} vs {want}"
        );
    }
}

/// Cites: EMU-3, EMU-9
#[test]
fn jitter_stays_in_range_with_the_uniform_mean() {
    let (delay, jitter) = (10 * MS, 2 * MS);
    for seed in SEEDS {
        let mut s = spec();
        s.delay = Some(Delay {
            delay_ns: delay,
            jitter_ns: jitter,
        });
        let mut l = Link::new(s, seed).unwrap();
        // One second apart: the order rule never binds.
        let n = 50_000;
        let d: Vec<i64> = run(&mut l, n, SECOND, 100)
            .iter()
            .map(|f| f.delay_ns)
            .collect();
        assert!(
            d.iter()
                .all(|x| (delay - jitter..=delay + jitter).contains(x))
        );
        let mean = d.iter().sum::<i64>() as f64 / n as f64;
        let k = (2 * jitter + 1) as f64;
        let sd = ((k * k - 1.0) / 12.0).sqrt() / (n as f64).sqrt();
        assert!(
            (mean - delay as f64).abs() <= 4.0 * sd,
            "seed {seed}: mean {mean}"
        );
    }
}

/// Cites: EMU-3
#[test]
fn without_reorder_delivery_is_fifo() {
    for seed in SEEDS {
        let mut s = spec();
        s.delay = Some(Delay {
            delay_ns: 10 * MS,
            jitter_ns: 9 * MS,
        });
        let mut l = Link::new(s, seed).unwrap();
        // 1 ms apart with 9 ms of jitter: candidates often overtake.
        let fates = run(&mut l, 10_000, MS, 100);
        let at: Vec<i64> = fates.iter().map(|f| f.outcome.unwrap()).collect();
        assert!(at.windows(2).all(|w| w[0] <= w[1]), "seed {seed}");
        assert!(fates.iter().all(|f| !f.reordered));
        // Never earlier than sent.
        assert!(fates.iter().all(|f| f.outcome.unwrap() >= f.send_ns));
    }
}

/// Cites: EMU-4
#[test]
fn throughput_settles_at_the_rate() {
    // 1 Mbit/s, a 12 500-byte bucket, 10 000 messages of 1 250 bytes at once.
    let mut s = spec();
    s.rate = Some(Rate {
        rate_bps: 1_000_000,
        burst_bytes: 12_500,
        queue_bytes: u64::MAX / 2,
    });
    let mut l = Link::new(s, 1).unwrap();
    let fates = run(&mut l, 10_000, 0, 1_250);
    let at: Vec<i64> = fates.iter().map(|f| f.outcome.unwrap()).collect();
    // The first ten fill the bucket and leave at once; then one every 10 ms.
    assert!(at[..10].iter().all(|t| *t == 0));
    for (i, t) in at.iter().enumerate().skip(10) {
        assert_eq!(*t, (i as i64 - 9) * 10 * MS, "message {i}");
    }
    assert_eq!(fates[10].rate_wait_ns, 10 * MS);
}

/// Cites: EMU-4
#[test]
fn a_full_queue_drops_and_drains() {
    let mut s = spec();
    s.rate = Some(Rate {
        rate_bps: 8_000,
        burst_bytes: 1_000,
        queue_bytes: 5_000,
    });
    let mut l = Link::new(s, 1).unwrap();
    let burst = run(&mut l, 20, 0, 1_000);
    let ok = burst.iter().filter(|f| f.outcome.is_ok()).count();
    let queue = burst
        .iter()
        .filter(|f| f.outcome == Err(DropCause::Queue))
        .count();
    // One leaves at once from the full bucket; five more fill the queue.
    assert_eq!((ok, queue), (6, 14));
    // Once the queue drains (5 s at 1 000 bytes/s), messages are accepted.
    assert!(l.transmit(6 * SECOND, 1_000).unwrap().outcome.is_ok());
}

/// Cites: EMU-8, EMU-3, EMU-9
#[test]
fn reorder_matches_its_rate_and_lets_later_messages_overtake() {
    for seed in SEEDS {
        let mut s = spec();
        s.delay = Some(Delay {
            delay_ns: 10 * MS,
            jitter_ns: 0,
        });
        s.reorder = Some(Reorder {
            reorder_ppm: 100_000,
            gap_ns: 5 * MS,
        });
        let mut l = Link::new(s, seed).unwrap();
        let n = 100_000;
        let fates = run(&mut l, n, MS, 100);
        let re: Vec<&Fate> = fates.iter().filter(|f| f.reordered).collect();
        assert!(
            within_4_sigma(re.len(), n, 0.1),
            "seed {seed}: {}",
            re.len()
        );
        assert!(re.iter().all(|f| f.delay_ns == 15 * MS));
        // A reordered message is overtaken by the next one, which is not held
        // behind it.
        let i = fates.iter().position(|f| f.reordered).unwrap();
        if let Some(next) = fates.get(i + 1).filter(|f| !f.reordered) {
            assert!(next.outcome.unwrap() < fates[i].outcome.unwrap());
        }
    }
}

/// Cites: EMU-7
#[test]
fn outage_windows_drop_or_hold() {
    let windows = |mode| {
        let mut s = spec();
        s.outage = Some(vec![Window {
            start_ns: SECOND,
            end_ns: 2 * SECOND,
            mode,
            cause: OutageCause::Handover,
        }]);
        Link::new(s, 1).unwrap()
    };
    let mut drop = windows(OutageMode::Drop);
    assert_eq!(
        drop.transmit(SECOND - 1, 1).unwrap().outcome,
        Ok(SECOND - 1)
    );
    assert_eq!(
        drop.transmit(SECOND, 1).unwrap().outcome,
        Err(DropCause::Outage)
    );
    assert_eq!(
        drop.transmit(2 * SECOND - 1, 1).unwrap().outcome,
        Err(DropCause::Outage)
    );
    assert_eq!(
        drop.transmit(2 * SECOND, 1).unwrap().outcome,
        Ok(2 * SECOND)
    );

    let mut hold = windows(OutageMode::Hold);
    let f = hold.transmit(SECOND + 300 * MS, 1).unwrap();
    assert_eq!(f.outcome, Ok(2 * SECOND));
    assert_eq!(f.hold_ns, 700 * MS);
}

/// Cites: EMU-1
#[test]
fn a_message_sent_out_of_order_is_refused() {
    let mut l = Link::new(spec(), 1).unwrap();
    assert_eq!(l.transmit(10, 1).unwrap().outcome, Ok(10));
    assert_eq!(l.transmit(10, 1).unwrap().outcome, Ok(10));
    assert_eq!(l.transmit(5, 1).unwrap_err().reason, "order");
}

/// Cites: EMU-1, EMU-2
#[test]
fn a_link_with_no_stages_delivers_at_once() {
    let mut l = Link::new(spec(), 7).unwrap();
    for f in run(&mut l, 100, 3, 1_000_000) {
        assert_eq!(f.outcome, Ok(f.send_ns));
        assert_eq!(
            (f.hold_ns, f.rate_wait_ns, f.delay_ns, f.reordered),
            (0, 0, 0, false)
        );
    }
}

/// Cites: EMU-2, EMU-9
#[test]
fn a_message_dropped_by_the_outage_is_not_offered_to_the_loss_stage() {
    let loss = Loss::Iid { loss_ppm: 300_000 };
    let mut with = spec();
    with.outage = Some(vec![Window {
        start_ns: 100,
        end_ns: 200,
        mode: OutageMode::Drop,
        cause: OutageCause::Scheduled,
    }]);
    with.loss = Some(loss);
    let mut without = spec();
    without.loss = Some(loss);
    let a = run(&mut Link::new(with, 9).unwrap(), 1_000, 1, 1);
    let b = run(&mut Link::new(without, 9).unwrap(), 1_000, 1, 1);
    // The loss outcomes of the messages that reach the loss stage on `a` are
    // the first outcomes of `b`, in order: the outage made no loss draws.
    let reached: Vec<bool> = a
        .iter()
        .filter(|f| f.outcome != Err(DropCause::Outage))
        .map(|f| f.outcome.is_err())
        .collect();
    let first: Vec<bool> = b
        .iter()
        .take(reached.len())
        .map(|f| f.outcome.is_err())
        .collect();
    assert_eq!(reached.len(), 900);
    assert_eq!(reached, first);
}

/// Cites: EMU-9
#[test]
fn each_stage_draws_from_its_own_sub_stream() {
    let make = |loss_ppm, jitter_ns| {
        let mut s = spec();
        s.loss = Some(Loss::Iid { loss_ppm });
        s.delay = Some(Delay {
            delay_ns: 10 * MS,
            jitter_ns,
        });
        Link::new(s, 11).unwrap()
    };
    // Changing the loss rate: the jitter of delivered messages is the same
    // sequence of draws.
    let jit = |fates: &[Fate]| -> Vec<i64> {
        fates
            .iter()
            .filter(|f| f.outcome.is_ok())
            .map(|f| f.delay_ns)
            .collect()
    };
    let low = run(&mut make(10_000, MS), 2_000, SECOND, 1);
    let high = run(&mut make(400_000, MS), 2_000, SECOND, 1);
    let (jl, jh) = (jit(&low), jit(&high));
    assert_eq!(jl[..jh.len()], jh[..]);
    // Changing the jitter: the losses are unchanged.
    let lost = |fates: &[Fate]| -> Vec<bool> { fates.iter().map(|f| f.outcome.is_err()).collect() };
    let small = run(&mut make(100_000, MS), 2_000, SECOND, 1);
    let large = run(&mut make(100_000, 5 * MS), 2_000, SECOND, 1);
    assert_eq!(lost(&small), lost(&large));
}

/// The fates of a fixed sequence on a link with every stage, for one seed: a
/// change to any stage's arithmetic or draws moves them (CON-5(a), EMU-9).
///
/// Cites: EMU-1, EMU-2, EMU-3, EMU-4, EMU-6, EMU-7, EMU-8, EMU-9
#[test]
fn golden_fates_for_one_seed() {
    let mut s = spec();
    s.outage = Some(vec![Window {
        start_ns: 40 * MS,
        end_ns: 50 * MS,
        mode: OutageMode::Hold,
        cause: OutageCause::Handover,
    }]);
    s.loss = Some(Loss::GilbertElliott {
        p_good_bad_ppm: 100_000,
        p_bad_good_ppm: 300_000,
        loss_good_ppm: 10_000,
        loss_bad_ppm: 500_000,
    });
    s.rate = Some(Rate {
        rate_bps: 2_000_000,
        burst_bytes: 3_000,
        queue_bytes: 20_000,
    });
    s.delay = Some(Delay {
        delay_ns: 20 * MS,
        jitter_ns: 5 * MS,
    });
    s.reorder = Some(Reorder {
        reorder_ppm: 200_000,
        gap_ns: 7 * MS,
    });
    let mut l = Link::new(s, 42).unwrap();
    let got: Vec<String> = (0..16)
        .map(|i| {
            let f = l.transmit(i * 4 * MS, 1_500).unwrap();
            match f.outcome {
                Ok(t) => format!("{t}{}", if f.reordered { "r" } else { "" }),
                Err(c) => format!("{c:?}"),
            }
        })
        .collect();
    assert_eq!(got, GOLDEN, "{got:?}");
}

const GOLDEN: [&str; 16] = [
    "24685319",
    "24685319",
    "31737653",
    "34243133",
    "34243133",
    "48893799",
    "48893799",
    "54756745",
    "64315106",
    "72379889",
    "77265079",
    "82311865",
    "Loss",
    "93952111r",
    "90201968",
    "99610736",
];
