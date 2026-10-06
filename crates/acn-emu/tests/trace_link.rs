//! Trace-driven links (SPEC 020 EMU-10 to EMU-12, EMU-22): the schedule the
//! 5G-IANA trace gives each direction, the rate integrated across segments,
//! draws by message index whatever the segment, and a replay scenario that
//! runs past the trace's period.
#![allow(clippy::unwrap_used, clippy::expect_used, clippy::panic)]

use std::path::{Path, PathBuf};

use acn_emu::link::{
    Direction, DropCause, Fate, Link, LinkModel as _, LinkSpec, Segment, TraceSchedule,
};
use acn_emu::scenario::load;

const MS: i64 = 1_000_000;
const SECOND: i64 = 1_000_000_000;

fn repo() -> PathBuf {
    Path::new(env!("CARGO_MANIFEST_DIR")).join("../..")
}

fn trace() -> acn_emu::trace::Trace {
    acn_emu::trace::load(&repo().join("scenarios/measured/5g-iana-2023-01-29"))
        .unwrap()
        .trace
}

fn traced(schedule: TraceSchedule) -> LinkSpec {
    let mut s = LinkSpec::new("radio", Direction::Up);
    s.trace = Some(schedule);
    s
}

fn seg(from_s: i64, rate_bps: u64, loss_ppm: u64, delay_ms: i64, jitter_ms: i64) -> Segment {
    Segment {
        from_ns: from_s * SECOND,
        loss_ppm,
        rate_bps,
        delay_ns: delay_ms * MS,
        jitter_ns: jitter_ms * MS,
        outage: rate_bps == 0,
    }
}

fn schedule(segments: Vec<Segment>, period_s: i64) -> TraceSchedule {
    TraceSchedule {
        segments,
        period_ns: period_s * SECOND,
        start_ns: 0,
        burst_bytes: 100,
        queue_bytes: 1 << 40,
    }
}

/// Cites: EMU-12
#[test]
fn the_5g_trace_gives_each_direction_its_schedule() {
    let t = trace();
    let up = TraceSchedule::from_trace(&t, Direction::Up, 0, 1_000, 10_000).unwrap();
    let down = TraceSchedule::from_trace(&t, Direction::Down, 0, 1_000, 10_000).unwrap();
    assert_eq!(up.segments.len(), 198);
    assert_eq!(up.period_ns, (5_121 + 26) * SECOND);
    // Sample 0: RTT avg 36 ms, stdev 13 ms, no loss, ul 2 403 kbps, dl 120 858.
    let s0 = up.segments[0];
    assert_eq!(
        (s0.from_ns, s0.delay_ns, s0.jitter_ns, s0.loss_ppm),
        (0, 18 * MS, 6_500_000, 0)
    );
    assert_eq!(
        (s0.rate_bps, down.segments[0].rate_bps),
        (2_403_000, 120_858_000)
    );
    // Every sample with total loss is an outage, at rate 0, in both directions.
    for (k, s) in t.samples.iter().enumerate() {
        let lost = s.loss >= 1.0;
        assert_eq!(
            up.segments[k].outage,
            lost || s.ul_kbps == 0.0,
            "sample {k}"
        );
        assert_eq!(
            down.segments[k].outage,
            lost || s.dl_kbps == 0.0,
            "sample {k}"
        );
        if up.segments[k].outage {
            assert_eq!(up.segments[k].rate_bps, 0);
        }
    }
    assert_eq!(up.segments.iter().filter(|s| s.outage).count(), 19);
    // A loss of 0.06 becomes 60 000 ppm.
    let k = t.samples.iter().position(|s| s.loss == 0.06).unwrap();
    assert_eq!(up.segments[k].loss_ppm, 60_000);
}

/// Cites: EMU-12, EMU-22
#[test]
fn the_offset_and_the_period_place_link_time_on_the_trace() {
    let t = trace();
    let mk = |start_s| {
        let s = TraceSchedule::from_trace(&t, Direction::Down, start_s, 256_000, 1 << 30).unwrap();
        Link::new(traced(s), 1).unwrap()
    };
    // At link time 0 with start 0, sample 0; one period later, sample 0 again.
    let mut l = mk(0);
    assert_eq!(l.transmit(0, 1).unwrap().sample, Some(0));
    assert_eq!(l.transmit(5_147 * SECOND, 1).unwrap().sample, Some(0));
    // With start_s = 25, link time 0 is trace time 25 s (sample 0, which runs
    // to 25 s exclusive, so sample 1).
    let mut l = mk(25);
    assert_eq!(l.transmit(0, 1).unwrap().sample, Some(1));
    // The last 14 samples are outages: a message sent then is dropped.
    let mut l = mk(0);
    let f = l.transmit(5_140 * SECOND, 1).unwrap();
    assert_eq!((f.outcome, f.sample), (Err(DropCause::Outage), Some(197)));
    // An offset at or beyond the period is refused.
    assert_eq!(
        TraceSchedule::from_trace(&t, Direction::Down, 5_147, 1, 1)
            .unwrap_err()
            .reason,
        "trace"
    );
}

/// Cites: EMU-10, EMU-11
#[test]
fn credit_accrues_across_segments_and_not_in_an_outage() {
    // 1 000 bytes/s for 1 s, an outage for 1 s, then 1 000 bytes/s; a 100-byte
    // bucket.
    let s = schedule(
        vec![
            seg(0, 8_000, 0, 0, 0),
            seg(1, 0, 0, 0, 0),
            seg(2, 8_000, 0, 0, 0),
        ],
        3,
    );
    let mut l = Link::new(traced(s), 1).unwrap();
    assert_eq!(l.transmit(0, 100).unwrap().outcome, Ok(0));
    // 100 bytes of credit take 0.1 s.
    assert_eq!(l.transmit(0, 1_000).unwrap().outcome, Ok(100 * MS));
    // Now 900 bytes are owed: 0.9 s to 1 s pays them, the outage adds nothing,
    // and the 100 bytes needed come from 2 s to 2.1 s.
    let f = l.transmit(0, 1_000).unwrap();
    assert_eq!(f.outcome, Ok(2_100 * MS));
    assert_eq!(f.rate_wait_ns, 2_100 * MS);
}

/// Cites: EMU-10, EMU-11
#[test]
fn a_long_wait_skips_whole_periods() {
    // 8 bytes/s in a 10 s period with 5 s of outage: 40 bytes per period.
    let s = schedule(vec![seg(0, 64, 0, 0, 0), seg(5, 0, 0, 0, 0)], 10);
    let mut l = Link::new(
        traced(TraceSchedule {
            burst_bytes: 8,
            ..s
        }),
        1,
    )
    .unwrap();
    assert_eq!(l.transmit(0, 8).unwrap().outcome, Ok(0));
    // A 400-byte message waits for a full 8-byte bucket (1 s), then leaves
    // the credit at -392 bytes.
    assert_eq!(l.transmit(0, 400).unwrap().outcome, Ok(SECOND));
    // The next 8 bytes need 400 bytes of accrual from 1 s: [1, 5) gives 32,
    // nine more periods give 360 (to 95 s), the outage [95, 100) nothing,
    // and the last 8 bytes take [100, 101).
    assert_eq!(l.transmit(0, 8).unwrap().outcome, Ok(101 * SECOND));
}

/// Cites: EMU-12, EMU-9
#[test]
fn each_segment_s_parameters_apply_and_draws_stay_by_index() {
    // Two segments of 1 s: the first lossless with 10 ms delay and 2 ms
    // jitter, the second with half the messages lost and 30 ms delay, no
    // jitter.
    let a = schedule(
        vec![seg(0, 1 << 40, 0, 10, 2), seg(1, 1 << 40, 500_000, 30, 0)],
        2,
    );
    let run = |s: TraceSchedule| -> Vec<Fate> {
        let mut l = Link::new(traced(s), 7).unwrap();
        (0..4_000).map(|i| l.transmit(i * MS, 1).unwrap()).collect()
    };
    let fa = run(a.clone());
    for f in &fa {
        let in_first = (f.send_ns % (2 * SECOND)) < SECOND;
        match f.outcome {
            Ok(_) if in_first => assert!((8 * MS..30 * MS).contains(&f.delay_ns)),
            Ok(_) => assert!(f.delay_ns >= 30 * MS),
            Err(c) => {
                assert_eq!(c, DropCause::Loss);
                assert!(!in_first);
            }
        }
    }
    let lost = fa.iter().filter(|f| f.outcome.is_err()).count();
    assert!((800..1_200).contains(&lost), "{lost} of 2 000");
    // Changing the second segment's loss leaves every message's jitter in the
    // first segment as it was: the delay draw is by index.
    let mut b = a;
    b.segments[1].loss_ppm = 0;
    let fb = run(b);
    for (x, y) in fa.iter().zip(&fb).take(1_000) {
        assert_eq!(x.outcome, y.outcome);
    }
}

/// Cites: EMU-12, EMU-22, EMU-20, EMU-21
#[test]
fn the_replay_scenario_runs_past_the_period() {
    let s = load(&repo().join("scenarios/synthetic/5g-iana-replay.toml")).unwrap();
    let mut links = s.build(3).unwrap();
    for l in &mut links {
        let mut drops = 0;
        let mut samples = Vec::new();
        // A 1 400-byte message every 10 s for two periods.
        for i in 0..1_030 {
            let f = l.transmit(i * 10 * SECOND, 1_400).unwrap();
            if f.outcome == Err(DropCause::Outage) {
                drops += 1;
            }
            samples.push(f.sample.unwrap());
        }
        assert!(drops > 0, "{}", l.spec().direction);
        // Both periods are replayed: the last sample, and the first sample
        // once in each period.
        assert!(samples.contains(&197));
        assert!(samples.iter().filter(|k| **k == 0).count() >= 2);
    }
}

/// Cites: EMU-22, EMU-21
#[test]
fn a_trace_reference_that_does_not_hold_is_refused() {
    let text =
        std::fs::read_to_string(repo().join("scenarios/synthetic/5g-iana-replay.toml")).unwrap();
    // The copy lives in a temporary directory, so its `dir` points back at the
    // repository's measured traces by an absolute path.
    let measured = repo().join("scenarios/measured").canonicalize().unwrap();
    let text = text.replace(
        "dir = \"../measured/",
        &format!("dir = \"{}/", measured.display()),
    );
    let dir_key = "5g-iana-2023-01-29\"\nblake3";
    assert!(text.contains(dir_key));
    let cases = [
        (
            "hash",
            text.replacen("blake3 = \"638293c8", "blake3 = \"738293c8", 1),
            "trace",
        ),
        (
            "missing",
            text.replacen(dir_key, "nowhere\"\nblake3", 1),
            "trace",
        ),
        (
            "offset",
            text.replacen("start_s = 0", "start_s = 5147", 1),
            "trace",
        ),
        (
            "extra stage",
            text.replacen(
                "start_s = 0",
                "start_s = 0\n\n[link.delay]\ndelay_us = 1\njitter_us = 0",
                1,
            ),
            "parse",
        ),
    ];
    for (what, t, reason) in cases {
        let tmp = tempfile::tempdir().unwrap();
        let path = tmp.path().join("5g-iana-replay.toml");
        std::fs::write(&path, t).unwrap();
        let e = load(&path).expect_err(what);
        assert_eq!(e.reason, reason, "{what}: {e}");
    }
    // A schedule with no positive rate is refused.
    let mut s = schedule(vec![seg(0, 0, 0, 0, 0)], 1);
    s.segments[0].outage = true;
    assert_eq!(Link::new(traced(s), 1).unwrap_err().reason, "trace");
}
