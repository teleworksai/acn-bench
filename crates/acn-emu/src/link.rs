//! Link models (SPEC 020 §2, EMU-1 to EMU-9): what one direction of a link does
//! to each message. A [`Link`] is a fixed pipeline of optional stages (outage,
//! loss, rate, delay, reorder) computed in exact integers. Each stage draws
//! from its own seeded sub-stream a fixed number of times per message index,
//! whatever happened to the message earlier in the pipeline, so the draws are a
//! schedule fixed by the seed before any traffic (CON-5(d)), and the sim engine
//! and the live proxy get the same fates from the same seed and send times.

use std::collections::VecDeque;
use std::fmt;

use rand_chacha::ChaCha20Rng;
use rand_core::Rng as _;

/// One million: probabilities are in parts per million.
pub const PPM: u64 = 1_000_000;
/// The largest time or duration a link handles, in nanoseconds (EMU-1).
pub const MAX_NS: i64 = 1 << 62;
/// Credit units per byte in the rate stage: bit-nanoseconds (EMU-4).
const UNITS_PER_BYTE: i128 = 8 * 1_000_000_000;

/// Why a link or a message was refused.
#[derive(Debug, Clone, PartialEq, Eq, thiserror::Error)]
#[error("{reason}: {message}")]
pub struct LinkError {
    /// `name` (a link name that is not a name), `parse` (a trace link with
    /// another stage), `range` (a parameter or a time out of range), `trace`
    /// (a trace that cannot drive a link), `order` (a message sent before the
    /// one before it) or `stream` (a sub-stream the seed cannot take).
    pub reason: &'static str,
    pub message: String,
}

fn refuse<T>(reason: &'static str, message: impl Into<String>) -> Result<T, LinkError> {
    Err(LinkError {
        reason,
        message: message.into(),
    })
}

/// `a + b`, refused when the sum leaves `[0, MAX_NS]` (EMU-1).
fn add(a: i64, b: i64, what: &str) -> Result<i64, LinkError> {
    match a.checked_add(b) {
        Some(t) if (0..=MAX_NS).contains(&t) => Ok(t),
        _ => refuse("range", format!("{what} takes a time beyond 2^62 ns")),
    }
}

/// Whether `s` is a name: lowercase ASCII letters, digits and `-` (EMU-20).
#[must_use]
pub fn is_name(s: &str) -> bool {
    !s.is_empty()
        && s.bytes()
            .all(|b| b.is_ascii_lowercase() || b.is_ascii_digit() || b == b'-')
}

/// A link's direction.
#[derive(Debug, Clone, Copy, PartialEq, Eq, PartialOrd, Ord)]
pub enum Direction {
    /// Client to server.
    Up,
    /// Server to client.
    Down,
}

impl Direction {
    /// The name TRC-15 records.
    #[must_use]
    pub fn as_str(self) -> &'static str {
        match self {
            Self::Up => "up",
            Self::Down => "down",
        }
    }
}

impl fmt::Display for Direction {
    fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
        f.write_str(self.as_str())
    }
}

/// What an outage window does to a message sent inside it (EMU-7).
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum OutageMode {
    Drop,
    Hold,
}

/// Why an outage happens, for TRC-15's `acn.scenario.outage` event.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum OutageCause {
    Handover,
    Scheduled,
}

/// One outage window `[start_ns, end_ns)`, from the link's origin.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub struct Window {
    pub start_ns: i64,
    pub end_ns: i64,
    pub mode: OutageMode,
    pub cause: OutageCause,
}

/// The loss stage (EMU-5, EMU-6).
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum Loss {
    Iid {
        loss_ppm: u64,
    },
    GilbertElliott {
        p_good_bad_ppm: u64,
        p_bad_good_ppm: u64,
        loss_good_ppm: u64,
        loss_bad_ppm: u64,
    },
}

/// The rate stage (EMU-4).
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub struct Rate {
    pub rate_bps: u64,
    pub burst_bytes: u64,
    pub queue_bytes: u64,
}

/// The delay stage (EMU-3).
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub struct Delay {
    pub delay_ns: i64,
    pub jitter_ns: i64,
}

/// The reorder stage (EMU-8).
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub struct Reorder {
    pub reorder_ppm: u64,
    pub gap_ns: i64,
}

/// One segment of a trace schedule: one trace sample's parameters (EMU-12).
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub struct Segment {
    /// Where the segment starts, in trace time.
    pub from_ns: i64,
    pub loss_ppm: u64,
    /// 0 in an outage segment.
    pub rate_bps: u64,
    pub delay_ns: i64,
    pub jitter_ns: i64,
    pub outage: bool,
}

/// A trace-driven schedule (EMU-10 to EMU-12): the trace's samples as
/// segments, repeating with the trace's period, read from `start_ns` on.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct TraceSchedule {
    /// Sorted by `from_ns`, the first at 0.
    pub segments: Vec<Segment>,
    pub period_ns: i64,
    pub start_ns: i64,
    pub burst_bytes: u64,
    pub queue_bytes: u64,
}

/// `v` rounded to the nearest integer, ties away from zero, refused outside
/// `[0, max]` (EMU-12).
fn rounded(v: f64, max: i64, what: &str) -> Result<i64, LinkError> {
    let r = v.round();
    if r.is_finite() && r >= 0.0 && r <= max as f64 {
        Ok(r as i64)
    } else {
        refuse("trace", format!("{what} {v} is outside [0, {max}]"))
    }
}

impl TraceSchedule {
    /// The schedule a measured trace gives a link in `direction` (EMU-12).
    pub fn from_trace(
        trace: &crate::trace::Trace,
        direction: Direction,
        start_s: u64,
        burst_bytes: u64,
        queue_bytes: u64,
    ) -> Result<Self, LinkError> {
        let mut segments = Vec::with_capacity(trace.samples.len());
        let mut rtts: Vec<Option<(i64, i64)>> = Vec::with_capacity(trace.samples.len());
        for s in &trace.samples {
            let kbps = match direction {
                Direction::Up => s.ul_kbps,
                Direction::Down => s.dl_kbps,
            };
            let rate = rounded(kbps * 1_000.0, MAX_NS, "a rate")?.cast_unsigned();
            let outage = s.loss >= 1.0 || rate == 0;
            // Half the RTT each way; a uniform half-range of `stdev * sqrt(3) / 2`
            // gives each direction a standard deviation of half the RTT's.
            let rtt = match s.rtt_ms {
                Some(r) => Some((
                    rounded(r.avg * 1e6 / 2.0, MAX_NS, "a delay")?,
                    rounded(r.stdev * 1e6 * 3.0_f64.sqrt() / 2.0, MAX_NS, "a jitter")?,
                )),
                None => None,
            };
            rtts.push(rtt);
            let (delay_ns, jitter_ns) = rtt.unwrap_or((0, 0));
            segments.push(Segment {
                from_ns: rounded(s.t_s * 1e9, MAX_NS, "a sample time")?,
                loss_ppm: rounded(s.loss * 1e6, 1_000_000, "a loss")?.cast_unsigned(),
                rate_bps: if outage { 0 } else { rate },
                delay_ns,
                jitter_ns,
                outage,
            });
        }
        // A sample with no round-trip time (an outage) carries the delay of the
        // nearest earlier sample that has one, or of the first later one, so a
        // message leaving the rate stage as an outage begins is still delayed.
        let mut carried = rtts.iter().flatten().next().copied();
        for (seg, rtt) in segments.iter_mut().zip(&rtts) {
            if rtt.is_some() {
                carried = *rtt;
            } else if let Some((d, j)) = carried {
                seg.delay_ns = d;
                seg.jitter_ns = j;
            }
        }
        let last = trace.samples.last().map_or(0.0, |s| s.t_s);
        let period_ns = rounded((last + trace.sample_interval_s) * 1e9, MAX_NS, "the period")?;
        let start_ns = i64::try_from(start_s)
            .ok()
            .and_then(|s| s.checked_mul(1_000_000_000))
            .unwrap_or(i64::MAX);
        let t = Self {
            segments,
            period_ns,
            start_ns,
            burst_bytes,
            queue_bytes,
        };
        t.validate()?;
        Ok(t)
    }

    /// Check the schedule's shape (EMU-11, EMU-12, EMU-22).
    pub fn validate(&self) -> Result<(), LinkError> {
        let Some(first) = self.segments.first() else {
            return refuse("trace", "a trace schedule has no segment");
        };
        if first.from_ns != 0
            || self
                .segments
                .windows(2)
                .any(|w| w[1].from_ns <= w[0].from_ns)
            || self
                .segments
                .last()
                .is_some_and(|s| s.from_ns >= self.period_ns)
        {
            return refuse("trace", "segments are not sorted within the period from 0");
        }
        if !(0..self.period_ns).contains(&self.start_ns) {
            return refuse("trace", "start_s is outside the trace's period");
        }
        if !self.segments.iter().any(|s| s.rate_bps > 0) {
            return refuse("trace", "the trace has no positive rate in this direction");
        }
        if self.burst_bytes == 0 || self.queue_bytes == 0 {
            return refuse("range", "burst and queue must be positive");
        }
        for (k, s) in self.segments.iter().enumerate() {
            if s.loss_ppm > PPM {
                return refuse("trace", format!("segment {k}: a loss above 10^6 ppm"));
            }
            if !(0..=MAX_NS).contains(&s.delay_ns) || !(0..=MAX_NS).contains(&s.jitter_ns) {
                return refuse(
                    "trace",
                    format!("segment {k}: a delay or jitter outside [0, 2^62]"),
                );
            }
            if s.outage != (s.rate_bps == 0) {
                return refuse(
                    "trace",
                    format!("segment {k}: an outage has rate 0, and only an outage"),
                );
            }
        }
        Ok(())
    }

    /// The segment in force at link time `tau >= 0`, and the link time at
    /// which it ends.
    fn at(&self, tau: i64) -> (usize, i64) {
        let u = (i128::from(tau) + i128::from(self.start_ns)) % i128::from(self.period_ns);
        // `u` is in [0, period), which fits i64.
        let u = i64::try_from(u).unwrap_or(0);
        let k = self
            .segments
            .partition_point(|s| s.from_ns <= u)
            .saturating_sub(1);
        let to = self
            .segments
            .get(k + 1)
            .map_or(self.period_ns, |s| s.from_ns);
        (k, tau.saturating_add(to - u))
    }

    /// Credit units gained in one whole period.
    fn per_period(&self) -> i128 {
        self.segments
            .iter()
            .enumerate()
            .map(|(k, s)| {
                let to = self
                    .segments
                    .get(k + 1)
                    .map_or(self.period_ns, |n| n.from_ns);
                i128::from(to - s.from_ns) * i128::from(s.rate_bps)
            })
            .sum()
    }

    /// Credit units gained over link time `[a, b)`, `a <= b` (EMU-11).
    fn gained(&self, a: i64, b: i64) -> i128 {
        let period = i128::from(self.period_ns);
        let whole = (i128::from(b) - i128::from(a)) / period;
        let mut total = whole * self.per_period();
        let mut t = a.saturating_add(i64::try_from(whole * period).unwrap_or(i64::MAX));
        while t < b {
            let (k, end) = self.at(t);
            let e = end.min(b);
            total += i128::from(e - t) * i128::from(self.segments[k].rate_bps);
            t = e;
        }
        total
    }

    /// The least `d >= 1` with `gained(start, start + d) >= x`, for `x > 0`
    /// (EMU-11); `None` beyond 2^62 ns.
    fn time_to(&self, start: i64, x: i128) -> Option<i64> {
        let per = self.per_period();
        // `validate` refused a schedule with no positive rate.
        let whole = (x - 1) / per;
        let mut rem = x - whole * per;
        let skip = i64::try_from(whole * i128::from(self.period_ns)).ok()?;
        let mut t = start.checked_add(skip)?;
        loop {
            if t > MAX_NS {
                return None;
            }
            let (k, end) = self.at(t);
            let r = i128::from(self.segments[k].rate_bps);
            let gain = i128::from(end - t) * r;
            if r > 0 && gain >= rem {
                let d = i64::try_from((rem + r - 1) / r).ok()?;
                return t.checked_add(d).filter(|e| *e <= MAX_NS).map(|e| e - start);
            }
            rem -= gain;
            t = end;
        }
    }
}

/// The parameters of one link: its name, direction and stages (EMU-2).
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct LinkSpec {
    pub name: String,
    pub direction: Direction,
    /// Sorted, non-overlapping windows; `None` for no outage stage.
    pub outage: Option<Vec<Window>>,
    pub loss: Option<Loss>,
    pub rate: Option<Rate>,
    pub delay: Option<Delay>,
    pub reorder: Option<Reorder>,
    /// A trace-driven schedule (EMU-12), in place of outage, loss, rate and
    /// delay.
    pub trace: Option<TraceSchedule>,
}

impl LinkSpec {
    /// A link with no stages.
    #[must_use]
    pub fn new(name: &str, direction: Direction) -> Self {
        Self {
            name: name.to_owned(),
            direction,
            outage: None,
            loss: None,
            rate: None,
            delay: None,
            reorder: None,
            trace: None,
        }
    }

    /// Check the name and every parameter against its range (EMU-1, EMU-3 to
    /// EMU-8).
    pub fn validate(&self) -> Result<(), LinkError> {
        if !is_name(&self.name) {
            return refuse(
                "name",
                format!("link `{}` is not a name (a-z, 0-9, -)", self.name),
            );
        }
        let ppm = |what: &str, v: u64| {
            if v > PPM {
                refuse("range", format!("{what} {v} is above 10^6 ppm"))
            } else {
                Ok(())
            }
        };
        let span = |what: &str, v: i64| {
            if (0..=MAX_NS).contains(&v) {
                Ok(())
            } else {
                refuse("range", format!("{what} {v} ns is outside [0, 2^62]"))
            }
        };
        if let Some(ws) = &self.outage {
            if ws.is_empty() {
                return refuse("range", "an outage stage has no window");
            }
            for (i, w) in ws.iter().enumerate() {
                span("an outage start", w.start_ns)?;
                span("an outage end", w.end_ns)?;
                if w.end_ns <= w.start_ns {
                    return refuse("range", format!("outage window {i} is empty"));
                }
                if i > 0 && w.start_ns < ws[i - 1].end_ns {
                    return refuse(
                        "range",
                        format!("outage window {i} is unsorted or overlaps the one before"),
                    );
                }
            }
        }
        match self.loss {
            Some(Loss::Iid { loss_ppm }) => ppm("loss_ppm", loss_ppm)?,
            Some(Loss::GilbertElliott {
                p_good_bad_ppm,
                p_bad_good_ppm,
                loss_good_ppm,
                loss_bad_ppm,
            }) => {
                ppm("p_good_bad_ppm", p_good_bad_ppm)?;
                ppm("p_bad_good_ppm", p_bad_good_ppm)?;
                ppm("loss_good_ppm", loss_good_ppm)?;
                ppm("loss_bad_ppm", loss_bad_ppm)?;
            }
            None => {}
        }
        if let Some(r) = self.rate
            && (r.rate_bps == 0 || r.burst_bytes == 0 || r.queue_bytes == 0)
        {
            return refuse("range", "rate, burst and queue must be positive");
        }
        if let Some(d) = self.delay {
            span("the delay", d.delay_ns)?;
            span("the jitter", d.jitter_ns)?;
        }
        if let Some(t) = &self.trace {
            if self.outage.is_some()
                || self.loss.is_some()
                || self.rate.is_some()
                || self.delay.is_some()
            {
                return refuse(
                    "parse",
                    format!(
                        "link {}: a trace link has no outage, loss, rate or delay stage",
                        self.name
                    ),
                );
            }
            t.validate()?;
        }
        if let Some(r) = self.reorder {
            ppm("reorder_ppm", r.reorder_ppm)?;
            span("the reorder gap", r.gap_ns)?;
            if r.gap_ns == 0 {
                return refuse("range", "the reorder gap must be positive");
            }
        }
        Ok(())
    }
}

/// Why a message was dropped (EMU-1).
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum DropCause {
    Outage,
    Loss,
    Queue,
}

/// What happened to one message (EMU-1, TRC-15, TRC-34).
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub struct Fate {
    pub send_ns: i64,
    /// `Ok(deliver_ns)`, or the cause of the drop.
    pub outcome: Result<i64, DropCause>,
    /// The index of the last outage window the message met, if any.
    pub window: Option<usize>,
    /// Time held by outage windows in `hold` mode.
    pub hold_ns: i64,
    /// Time spent waiting for the rate limiter.
    pub rate_wait_ns: i64,
    /// Delay applied after the rate stage, reorder gap included.
    pub delay_ns: i64,
    /// Whether the reorder stage selected the message.
    pub reordered: bool,
    /// The trace sample in force when the message was sent (EMU-12).
    pub sample: Option<usize>,
}

impl Fate {
    fn new(send_ns: i64) -> Self {
        Self {
            send_ns,
            outcome: Ok(send_ns),
            window: None,
            hold_ns: 0,
            rate_wait_ns: 0,
            delay_ns: 0,
            reordered: false,
            sample: None,
        }
    }

    fn dropped(mut self, cause: DropCause) -> Self {
        self.outcome = Err(cause);
        self
    }
}

/// A link model (EMU-1).
pub trait LinkModel {
    /// The fate of a message of `bytes` sent at `send_ns`. Messages are offered
    /// in non-decreasing order of `send_ns`.
    fn transmit(&mut self, send_ns: i64, bytes: u64) -> Result<Fate, LinkError>;
}

/// A uniform draw over `0..n` (n > 0), exact: rejection sampling on 64-bit
/// outputs, as EMU-9 fixes it.
pub fn below(rng: &mut ChaCha20Rng, n: u64) -> u64 {
    let zone = u64::MAX - (u64::MAX % n);
    loop {
        let x = rng.next_u64();
        if x < zone {
            return x % n;
        }
    }
}

/// True with probability `ppm / 10^6`, from one draw (EMU-9).
pub fn chance(rng: &mut ChaCha20Rng, ppm: u64) -> bool {
    below(rng, PPM) < ppm
}

/// The rate a bucket accrues at: fixed, or a trace's schedule (EMU-11).
#[derive(Debug, Clone)]
enum RateSource {
    Fixed(u64),
    Trace(TraceSchedule),
}

impl RateSource {
    /// Credit units gained over `[a, b)`.
    fn gained(&self, a: i64, b: i64) -> i128 {
        if b <= a {
            return 0;
        }
        match self {
            Self::Fixed(r) => i128::from(b - a) * i128::from(*r),
            Self::Trace(s) => s.gained(a, b),
        }
    }

    /// The least `d >= 1` with `gained(start, start + d) >= x`, for `x > 0`;
    /// `None` beyond 2^62 ns.
    fn time_to(&self, start: i64, x: i128) -> Option<i64> {
        match self {
            Self::Fixed(r) => {
                let r = i128::from(*r);
                i64::try_from((x + r - 1) / r).ok()
            }
            Self::Trace(s) => s.time_to(start, x),
        }
    }
}

/// The token bucket of EMU-4 and EMU-11.
#[derive(Debug, Clone)]
struct Bucket {
    rate: RateSource,
    burst_bytes: u64,
    queue_bytes: u64,
    /// Credit in bit-nanoseconds at `at_ns`; `None` before the first message,
    /// when the bucket is full.
    credit: i128,
    at_ns: Option<i64>,
    /// The last departure time.
    last_ns: Option<i64>,
    /// Accepted messages not yet departed: (departure, bytes).
    queue: VecDeque<(i64, u64)>,
    backlog: u128,
}

impl Bucket {
    fn new(rate: RateSource, burst_bytes: u64, queue_bytes: u64) -> Self {
        Self {
            rate,
            burst_bytes,
            queue_bytes,
            credit: i128::from(burst_bytes) * UNITS_PER_BYTE,
            at_ns: None,
            last_ns: None,
            queue: VecDeque::new(),
            backlog: 0,
        }
    }

    fn cap(&self) -> i128 {
        i128::from(self.burst_bytes) * UNITS_PER_BYTE
    }

    /// The credit at `t`, no earlier than `at_ns`.
    fn credit_at(&self, t: i64) -> i128 {
        match self.at_ns {
            None => self.credit,
            Some(at) => (self.credit + self.rate.gained(at, t)).min(self.cap()),
        }
    }

    /// The departure time of `bytes` arriving at `t`, or `None` for a tail drop.
    fn admit(&mut self, t: i64, bytes: u64) -> Result<Option<i64>, LinkError> {
        while self.queue.front().is_some_and(|(d, _)| *d <= t) {
            if let Some((_, b)) = self.queue.pop_front() {
                self.backlog -= u128::from(b);
            }
        }
        let start = self.last_ns.map_or(t, |l| t.max(l));
        let have = self.credit_at(start);
        let need = i128::from(bytes) * UNITS_PER_BYTE;
        let threshold = need.min(self.cap());
        let depart = if have >= threshold {
            Some(start)
        } else {
            self.rate
                .time_to(start, threshold - have)
                .and_then(|w| start.checked_add(w))
        };
        let Some(depart) = depart.filter(|d| *d <= MAX_NS) else {
            return refuse("range", "the rate stage takes a time beyond 2^62 ns");
        };
        if depart > t && self.backlog + u128::from(bytes) > u128::from(self.queue_bytes) {
            return Ok(None);
        }
        let at_depart = (have + self.rate.gained(start, depart)).min(self.cap());
        self.credit = at_depart - need;
        self.at_ns = Some(depart);
        self.last_ns = Some(depart);
        if depart > t {
            self.queue.push_back((depart, bytes));
            self.backlog += u128::from(bytes);
        }
        Ok(Some(depart))
    }
}

/// The Gilbert–Elliott state.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
enum GeState {
    Good,
    Bad,
}

/// One message's entry in the impairment schedule (EMU-9, EMU-12).
#[derive(Debug, Clone, Copy)]
struct Draws {
    /// A static loss stage's decision.
    lost: bool,
    /// A trace link's loss draw over `0..10^6`, compared with the segment's.
    loss_draw: Option<u64>,
    /// A static delay stage's jitter.
    jitter_ns: i64,
    /// A trace link's raw 64-bit jitter draw.
    jitter_raw: Option<u64>,
    selected: bool,
}

/// A link built from a [`LinkSpec`] under a replicate seed (EMU-1, EMU-9).
#[derive(Debug, Clone)]
pub struct Link {
    spec: LinkSpec,
    loss_rng: Option<ChaCha20Rng>,
    delay_rng: Option<ChaCha20Rng>,
    reorder_rng: Option<ChaCha20Rng>,
    ge: GeState,
    bucket: Option<Bucket>,
    /// The last send time offered.
    last_send_ns: Option<i64>,
    /// The delivery time of the last delivered message that was not selected.
    last_fifo_ns: Option<i64>,
}

/// The sub-stream of one stage of a link (EMU-9).
fn stream(seed: u64, spec: &LinkSpec, stage: &str) -> Result<ChaCha20Rng, LinkError> {
    let name = format!("link.{}.{}.{stage}", spec.name, spec.direction);
    acn_trace::identity::substream_rng(seed, &name).map_err(|e| LinkError {
        reason: "stream",
        message: e.to_string(),
    })
}

impl Link {
    /// The link `spec` describes, drawing from sub-streams of `replicate_seed`.
    pub fn new(spec: LinkSpec, replicate_seed: u64) -> Result<Self, LinkError> {
        spec.validate()?;
        let traced = spec.trace.is_some();
        let jitter = traced || spec.delay.is_some_and(|d| d.jitter_ns > 0);
        let bucket = match (&spec.trace, spec.rate) {
            (Some(t), _) => Some(Bucket::new(
                RateSource::Trace(t.clone()),
                t.burst_bytes,
                t.queue_bytes,
            )),
            (None, Some(r)) => Some(Bucket::new(
                RateSource::Fixed(r.rate_bps),
                r.burst_bytes,
                r.queue_bytes,
            )),
            (None, None) => None,
        };
        Ok(Self {
            loss_rng: (traced || spec.loss.is_some())
                .then(|| stream(replicate_seed, &spec, "loss"))
                .transpose()?,
            delay_rng: jitter
                .then(|| stream(replicate_seed, &spec, "delay"))
                .transpose()?,
            reorder_rng: spec
                .reorder
                .map(|_| stream(replicate_seed, &spec, "reorder"))
                .transpose()?,
            ge: GeState::Good,
            bucket,
            last_send_ns: None,
            last_fifo_ns: None,
            spec,
        })
    }

    /// The parameters this link was built from.
    #[must_use]
    pub fn spec(&self) -> &LinkSpec {
        &self.spec
    }

    /// This message's draws from every stage present (EMU-9), taken before
    /// the pipeline runs so that they depend on the message index alone.
    fn draw(&mut self) -> Draws {
        let traced = self.spec.trace.is_some();
        let mut d = Draws {
            lost: false,
            loss_draw: None,
            jitter_ns: 0,
            jitter_raw: None,
            selected: false,
        };
        if let Some(rng) = self.loss_rng.as_mut() {
            match self.spec.loss {
                _ if traced => d.loss_draw = Some(below(rng, PPM)),
                Some(Loss::Iid { loss_ppm }) => d.lost = chance(rng, loss_ppm),
                Some(Loss::GilbertElliott {
                    p_good_bad_ppm,
                    p_bad_good_ppm,
                    loss_good_ppm,
                    loss_bad_ppm,
                }) => {
                    let flip = below(rng, PPM);
                    let drop = below(rng, PPM);
                    self.ge = match self.ge {
                        GeState::Good if flip < p_good_bad_ppm => GeState::Bad,
                        GeState::Bad if flip < p_bad_good_ppm => GeState::Good,
                        s => s,
                    };
                    d.lost = drop
                        < match self.ge {
                            GeState::Good => loss_good_ppm,
                            GeState::Bad => loss_bad_ppm,
                        };
                }
                None => {}
            }
        }
        if let Some(rng) = self.delay_rng.as_mut() {
            if traced {
                d.jitter_raw = Some(rng.next_u64());
            } else if let Some(delay) = self.spec.delay {
                // `jitter_ns <= 2^62`, so the span fits in u64 and the draw in i64.
                let span = 2 * delay.jitter_ns.unsigned_abs() + 1;
                d.jitter_ns = below(rng, span).cast_signed() - delay.jitter_ns;
            }
        }
        if let (Some(r), Some(rng)) = (self.spec.reorder, self.reorder_rng.as_mut()) {
            d.selected = chance(rng, r.reorder_ppm);
        }
        d
    }
}

/// The jitter of a raw 64-bit draw over `[-jitter_ns, jitter_ns]` (EMU-12).
fn scaled_jitter(raw: u64, jitter_ns: i64) -> i64 {
    let span = u128::from(2 * jitter_ns.unsigned_abs() + 1);
    let k = (u128::from(raw) * span) >> 64;
    // `k < span <= 2^63 + 1` and `jitter_ns <= 2^62`: the result is in
    // `[-jitter_ns, jitter_ns]`, computed in i128 so that nothing saturates.
    let j = i128::try_from(k).unwrap_or(0) - i128::from(jitter_ns);
    i64::try_from(j).unwrap_or(0)
}

impl LinkModel for Link {
    fn transmit(&mut self, send_ns: i64, bytes: u64) -> Result<Fate, LinkError> {
        if !(0..=MAX_NS).contains(&send_ns) {
            return refuse(
                "range",
                format!("send time {send_ns} ns is outside [0, 2^62]"),
            );
        }
        if let Some(last) = self.last_send_ns.filter(|l| send_ns < *l) {
            return refuse(
                "order",
                format!("a message sent at {send_ns} ns follows one sent at {last} ns"),
            );
        }
        self.last_send_ns = Some(send_ns);
        let draws = self.draw();
        let mut fate = Fate::new(send_ns);
        let mut t = send_ns;

        // Outage (EMU-7): a hold moves the message to the window's end, where
        // the stage applies again. A trace's outage segments drop (EMU-12).
        if let Some(ws) = &self.spec.outage {
            while let Some(i) = ws.iter().position(|w| w.start_ns <= t && t < w.end_ns) {
                fate.window = Some(i);
                match ws[i].mode {
                    OutageMode::Drop => return Ok(fate.dropped(DropCause::Outage)),
                    OutageMode::Hold => {
                        fate.hold_ns += ws[i].end_ns - t;
                        t = ws[i].end_ns;
                    }
                }
            }
        }
        if let Some(tr) = &self.spec.trace {
            let (k, _) = tr.at(t);
            fate.sample = Some(k);
            if tr.segments[k].outage {
                return Ok(fate.dropped(DropCause::Outage));
            }
        }

        // Loss (EMU-5, EMU-6, EMU-12).
        let lost = match (&self.spec.trace, draws.loss_draw) {
            (Some(tr), Some(x)) => x < tr.segments[tr.at(t).0].loss_ppm,
            _ => draws.lost,
        };
        if lost {
            return Ok(fate.dropped(DropCause::Loss));
        }

        // Rate (EMU-4, EMU-11).
        if let Some(bucket) = self.bucket.as_mut() {
            let Some(depart) = bucket.admit(t, bytes)? else {
                return Ok(fate.dropped(DropCause::Queue));
            };
            fate.rate_wait_ns = depart - t;
            t = depart;
        }

        // Delay (EMU-3, EMU-12) and reorder (EMU-8).
        let delay = match (&self.spec.trace, draws.jitter_raw) {
            (Some(tr), Some(raw)) => {
                let s = &tr.segments[tr.at(t).0];
                Some((s.delay_ns, scaled_jitter(raw, s.jitter_ns)))
            }
            _ => self.spec.delay.map(|d| (d.delay_ns, draws.jitter_ns)),
        };
        let candidate = match delay {
            Some((delay_ns, j)) => add(t, delay_ns, "the delay")?
                .checked_add(j)
                .map_or(t, |c| c.max(t)),
            None => t,
        };
        let in_order = self.last_fifo_ns.map_or(candidate, |f| candidate.max(f));
        let deliver = match (draws.selected, self.spec.reorder) {
            (true, Some(r)) => add(in_order, r.gap_ns, "the reorder gap")?,
            _ => {
                self.last_fifo_ns = Some(in_order);
                in_order
            }
        };
        fate.reordered = draws.selected;
        fate.delay_ns = deliver - t;
        fate.outcome = Ok(deliver);
        Ok(fate)
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    fn rated(rate_bps: u64, burst_bytes: u64, queue_bytes: u64) -> Link {
        let mut s = LinkSpec::new("l", Direction::Up);
        s.rate = Some(Rate {
            rate_bps,
            burst_bytes,
            queue_bytes,
        });
        Link::new(s, 1).unwrap()
    }

    /// Cites: EMU-4
    #[test]
    fn a_burst_within_the_bucket_leaves_at_once() {
        let mut l = rated(8_000, 1_000, 10_000);
        for _ in 0..4 {
            let f = l.transmit(0, 250).unwrap();
            assert_eq!(f.outcome, Ok(0));
            assert_eq!(f.rate_wait_ns, 0);
        }
        // The bucket is empty: 250 bytes at 8 000 bit/s take 250 ms.
        let f = l.transmit(0, 250).unwrap();
        assert_eq!(f.outcome, Ok(250_000_000));
    }

    /// Cites: EMU-4
    #[test]
    fn a_message_larger_than_the_bucket_waits_for_a_full_bucket_then_owes() {
        let mut l = rated(8_000, 100, 100_000);
        // Full bucket of 100 bytes: the first 1 000-byte message leaves at once
        // and leaves the credit at -900 bytes.
        assert_eq!(l.transmit(0, 1_000).unwrap().outcome, Ok(0));
        // The next needs 100 bytes of credit: 1 000 bytes of accrual, 1 s.
        assert_eq!(l.transmit(0, 100).unwrap().outcome, Ok(1_000_000_000));
    }

    /// Cites: EMU-4
    #[test]
    fn the_queue_limit_tail_drops() {
        let mut l = rated(8_000, 100, 300);
        assert_eq!(l.transmit(0, 100).unwrap().outcome, Ok(0));
        // The bucket is empty: three 100-byte messages queue, leaving at 100,
        // 200 and 300 ms; a fourth would make the backlog 400 bytes.
        assert!(l.transmit(0, 100).unwrap().outcome.is_ok());
        assert!(l.transmit(0, 100).unwrap().outcome.is_ok());
        assert!(l.transmit(0, 100).unwrap().outcome.is_ok());
        assert_eq!(l.transmit(0, 100).unwrap().outcome, Err(DropCause::Queue));
    }

    /// Cites: EMU-4
    #[test]
    fn departure_rounds_up_to_the_next_nanosecond() {
        // 1 byte at 3 bit/s: 8/3 s.
        let mut l = rated(3, 1, 10);
        assert_eq!(l.transmit(0, 1).unwrap().outcome, Ok(0));
        assert_eq!(l.transmit(0, 1).unwrap().outcome, Ok(2_666_666_667));
    }

    /// Cites: EMU-4
    #[test]
    fn a_message_larger_than_the_queue_leaves_at_once_from_a_full_bucket() {
        let mut l = rated(8_000, 1_000, 100);
        assert_eq!(l.transmit(0, 500).unwrap().outcome, Ok(0));
        // Now the bucket holds 500 bytes: a second 500 leaves at once too, and
        // a third, which would wait, is larger than the queue.
        assert_eq!(l.transmit(0, 500).unwrap().outcome, Ok(0));
        assert_eq!(l.transmit(0, 500).unwrap().outcome, Err(DropCause::Queue));
    }

    /// Cites: EMU-4
    #[test]
    fn the_bucket_refills_only_to_its_cap() {
        let mut l = rated(8_000, 1_000, 10_000);
        assert_eq!(l.transmit(0, 1_000).unwrap().outcome, Ok(0));
        // 100 s idle refills 1 000 bytes, not 100 000: the second of two
        // 1 000-byte messages waits a full second.
        let s = 100_000_000_000;
        assert_eq!(l.transmit(s, 1_000).unwrap().outcome, Ok(s));
        assert_eq!(l.transmit(s, 1_000).unwrap().outcome, Ok(s + 1_000_000_000));
    }

    /// Cites: EMU-4
    #[test]
    fn a_message_departing_now_is_not_backlog_and_a_tail_drop_takes_no_credit() {
        let mut l = rated(8_000, 100, 200);
        assert_eq!(l.transmit(0, 100).unwrap().outcome, Ok(0));
        assert_eq!(l.transmit(0, 100).unwrap().outcome, Ok(100_000_000));
        assert_eq!(l.transmit(0, 100).unwrap().outcome, Ok(200_000_000));
        assert_eq!(l.transmit(0, 100).unwrap().outcome, Err(DropCause::Queue));
        // At 100 ms the message leaving then is gone: the backlog is 100 bytes,
        // and the dropped message took no credit, so this one leaves at 300 ms.
        assert_eq!(
            l.transmit(100_000_000, 100).unwrap().outcome,
            Ok(300_000_000)
        );
    }

    /// Cites: EMU-9
    #[test]
    fn draws_follow_the_pinned_algorithm() {
        let mut a = acn_trace::identity::substream_rng(5, "t").unwrap();
        let mut b = a.clone();
        let x = b.next_u64();
        // `x` is far below the rejection zone for n = 10^6, so it is used.
        assert_eq!(below(&mut a, PPM), x % PPM);
        let v = x % PPM;
        let mut c = acn_trace::identity::substream_rng(5, "t").unwrap();
        assert!(!chance(&mut c, v));
        let mut c = acn_trace::identity::substream_rng(5, "t").unwrap();
        assert!(chance(&mut c, v + 1));
        // For n just above 2^63, only outputs below n are kept: no modulo bias.
        let n = (1_u64 << 63) + 1;
        let mut r = acn_trace::identity::substream_rng(6, "t").unwrap();
        for _ in 0..64 {
            let mut probe = r.clone();
            let raw = probe.next_u64();
            let got = below(&mut r, n);
            if raw < n {
                assert_eq!(got, raw);
            }
        }
    }

    /// Cites: EMU-12, EMU-9
    #[test]
    fn scaled_jitter_is_pinned_at_its_extremes() {
        assert_eq!(scaled_jitter(0, 0), 0);
        assert_eq!(scaled_jitter(u64::MAX, 0), 0);
        assert_eq!(scaled_jitter(0, 5), -5);
        assert_eq!(scaled_jitter(u64::MAX, 5), 5);
        // 2^63 is the middle of the range: 11 * 2^63 / 2^64 = 5, minus 5.
        assert_eq!(scaled_jitter(1 << 63, 5), 0);
        assert_eq!(scaled_jitter(0, MAX_NS), -MAX_NS);
        assert_eq!(scaled_jitter(u64::MAX, MAX_NS), MAX_NS);
    }

    /// Cites: EMU-1
    #[test]
    fn times_beyond_2_62_are_refused() {
        let mut l = Link::new(LinkSpec::new("l", Direction::Up), 1).unwrap();
        assert_eq!(l.transmit(MAX_NS + 1, 1).unwrap_err().reason, "range");
        assert_eq!(l.transmit(-1, 1).unwrap_err().reason, "range");
        let mut s = LinkSpec::new("l", Direction::Up);
        s.delay = Some(Delay {
            delay_ns: MAX_NS,
            jitter_ns: 0,
        });
        let mut l = Link::new(s, 1).unwrap();
        assert_eq!(l.transmit(1, 1).unwrap_err().reason, "range");
        let mut s = LinkSpec::new("l", Direction::Up);
        s.delay = Some(Delay {
            delay_ns: 0,
            jitter_ns: MAX_NS + 1,
        });
        assert_eq!(Link::new(s, 1).unwrap_err().reason, "range");
        // A rate wait beyond the limit: a huge message leaves from a full
        // bucket and owes, and the next one would wait past 2^62 ns.
        let mut l = rated(1, 1, u64::MAX);
        assert!(l.transmit(0, u64::MAX / 2).is_ok());
        assert_eq!(l.transmit(0, 1).unwrap_err().reason, "range");
        // A name that is not a name.
        assert_eq!(
            Link::new(LinkSpec::new("a.b", Direction::Up), 1)
                .unwrap_err()
                .reason,
            "name"
        );
    }
}
