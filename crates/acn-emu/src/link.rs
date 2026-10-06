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
    /// `name` (a link name that is not a name), `range` (a parameter or a time
    /// out of range), `order` (a message sent before the one before it) or
    /// `stream` (a sub-stream the seed cannot take).
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

/// The token bucket of EMU-4.
#[derive(Debug, Clone)]
struct Bucket {
    rate: Rate,
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
    fn new(rate: Rate) -> Self {
        Self {
            rate,
            credit: i128::from(rate.burst_bytes) * UNITS_PER_BYTE,
            at_ns: None,
            last_ns: None,
            queue: VecDeque::new(),
            backlog: 0,
        }
    }

    fn cap(&self) -> i128 {
        i128::from(self.rate.burst_bytes) * UNITS_PER_BYTE
    }

    /// The credit at `t`, no earlier than `at_ns`.
    fn credit_at(&self, t: i64) -> i128 {
        match self.at_ns {
            None => self.credit,
            Some(at) => {
                let gained = i128::from(t - at) * i128::from(self.rate.rate_bps);
                (self.credit + gained).min(self.cap())
            }
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
        let rate = i128::from(self.rate.rate_bps);
        let wait = if have >= threshold {
            0
        } else {
            (threshold - have + rate - 1) / rate
        };
        let depart = i64::try_from(wait)
            .ok()
            .and_then(|w| start.checked_add(w))
            .filter(|d| *d <= MAX_NS);
        let Some(depart) = depart else {
            return refuse("range", "the rate stage takes a time beyond 2^62 ns");
        };
        if depart > t && self.backlog + u128::from(bytes) > u128::from(self.rate.queue_bytes) {
            return Ok(None);
        }
        let at_depart = (have + i128::from(depart - start) * rate).min(self.cap());
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

/// One message's entry in the impairment schedule (EMU-9).
#[derive(Debug, Clone, Copy)]
struct Draws {
    lost: bool,
    jitter_ns: i64,
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
        let jitter = spec.delay.is_some_and(|d| d.jitter_ns > 0);
        Ok(Self {
            loss_rng: spec
                .loss
                .map(|_| stream(replicate_seed, &spec, "loss"))
                .transpose()?,
            delay_rng: jitter
                .then(|| stream(replicate_seed, &spec, "delay"))
                .transpose()?,
            reorder_rng: spec
                .reorder
                .map(|_| stream(replicate_seed, &spec, "reorder"))
                .transpose()?,
            ge: GeState::Good,
            bucket: spec.rate.map(Bucket::new),
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
        let lost = match (self.spec.loss, self.loss_rng.as_mut()) {
            (Some(Loss::Iid { loss_ppm }), Some(rng)) => chance(rng, loss_ppm),
            (
                Some(Loss::GilbertElliott {
                    p_good_bad_ppm,
                    p_bad_good_ppm,
                    loss_good_ppm,
                    loss_bad_ppm,
                }),
                Some(rng),
            ) => {
                let flip = below(rng, PPM);
                let drop = below(rng, PPM);
                self.ge = match self.ge {
                    GeState::Good if flip < p_good_bad_ppm => GeState::Bad,
                    GeState::Bad if flip < p_bad_good_ppm => GeState::Good,
                    s => s,
                };
                drop < match self.ge {
                    GeState::Good => loss_good_ppm,
                    GeState::Bad => loss_bad_ppm,
                }
            }
            _ => false,
        };
        let jitter_ns = match (self.spec.delay, self.delay_rng.as_mut()) {
            (Some(d), Some(rng)) => {
                // `jitter_ns <= 2^62`, so the span fits in u64 and the draw in i64.
                let span = 2 * d.jitter_ns.unsigned_abs() + 1;
                below(rng, span).cast_signed() - d.jitter_ns
            }
            _ => 0,
        };
        let selected = match (self.spec.reorder, self.reorder_rng.as_mut()) {
            (Some(r), Some(rng)) => chance(rng, r.reorder_ppm),
            _ => false,
        };
        Draws {
            lost,
            jitter_ns,
            selected,
        }
    }
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
        // the stage applies again.
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

        // Loss (EMU-5, EMU-6).
        if draws.lost {
            return Ok(fate.dropped(DropCause::Loss));
        }

        // Rate (EMU-4).
        if let Some(bucket) = self.bucket.as_mut() {
            let Some(depart) = bucket.admit(t, bytes)? else {
                return Ok(fate.dropped(DropCause::Queue));
            };
            fate.rate_wait_ns = depart - t;
            t = depart;
        }

        // Delay (EMU-3) and reorder (EMU-8).
        let candidate = match self.spec.delay {
            Some(d) => add(t, d.delay_ns, "the delay")?
                .checked_add(draws.jitter_ns)
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
