//! Link models (SPEC 020 §2, EMU-1 to EMU-9): what one direction of a link does
//! to each message. A [`Link`] is a fixed pipeline of optional stages (outage,
//! loss, rate, delay, reorder) computed in exact integers from per-stage seeded
//! sub-streams, so the sim engine and the live proxy get the same fates from
//! the same seed.

use std::collections::VecDeque;
use std::fmt;

use rand_chacha::ChaCha20Rng;
use rand_core::Rng as _;

/// One million: probabilities are in parts per million.
pub const PPM: u64 = 1_000_000;
/// Credit units per byte in the rate stage: bit-nanoseconds (EMU-4).
const UNITS_PER_BYTE: i128 = 8 * 1_000_000_000;

/// Why a link or a message was refused.
#[derive(Debug, Clone, PartialEq, Eq, thiserror::Error)]
#[error("{reason}: {message}")]
pub struct LinkError {
    /// `range` (a parameter out of range), `order` (a message sent before the
    /// one before it) or `stream` (a sub-stream name the seed cannot take).
    pub reason: &'static str,
    pub message: String,
}

fn refuse<T>(reason: &'static str, message: impl Into<String>) -> Result<T, LinkError> {
    Err(LinkError {
        reason,
        message: message.into(),
    })
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

/// One outage window `[start_ns, end_ns)`.
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

    /// Check every parameter against its range (EMU-3 to EMU-8).
    pub fn validate(&self) -> Result<(), LinkError> {
        let ppm = |what: &str, v: u64| {
            if v > PPM {
                refuse("range", format!("{what} {v} is above 10^6 ppm"))
            } else {
                Ok(())
            }
        };
        if let Some(ws) = &self.outage {
            if ws.is_empty() {
                return refuse("range", "an outage stage has no window");
            }
            for (i, w) in ws.iter().enumerate() {
                if w.start_ns < 0 || w.end_ns <= w.start_ns {
                    return refuse("range", format!("outage window {i} is empty or negative"));
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
        if let Some(d) = self.delay
            && (d.delay_ns < 0 || d.jitter_ns < 0)
        {
            return refuse("range", "delay and jitter must not be negative");
        }
        if let Some(r) = self.reorder {
            ppm("reorder_ppm", r.reorder_ppm)?;
            if r.gap_ns <= 0 {
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

/// What happened to one message (EMU-1, TRC-15).
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub struct Fate {
    pub send_ns: i64,
    /// `Ok(deliver_ns)`, or the cause of the drop.
    pub outcome: Result<i64, DropCause>,
    /// Time held by an outage window in `hold` mode.
    pub hold_ns: i64,
    /// Time spent waiting for the rate limiter.
    pub rate_wait_ns: i64,
    /// Delay applied after the rate stage, reorder gap included.
    pub delay_ns: i64,
    pub reordered: bool,
}

impl Fate {
    fn new(send_ns: i64) -> Self {
        Self {
            send_ns,
            outcome: Ok(send_ns),
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

/// A uniform draw over `0..n` (n > 0), exact: rejection sampling on 64 bits.
fn below(rng: &mut ChaCha20Rng, n: u64) -> u64 {
    let zone = u64::MAX - (u64::MAX % n);
    loop {
        let x = rng.next_u64();
        if x < zone {
            return x % n;
        }
    }
}

/// True with probability `ppm / 10^6`, from one draw (EMU-9).
fn chance(rng: &mut ChaCha20Rng, ppm: u64) -> bool {
    below(rng, PPM) < ppm
}

/// The token bucket of EMU-4.
#[derive(Debug, Clone)]
struct Bucket {
    rate: Rate,
    /// Credit in bit-nanoseconds at `at_ns`.
    credit: i128,
    at_ns: i64,
    /// The last departure time.
    last_ns: i64,
    /// Accepted messages not yet departed: (departure, bytes).
    queue: VecDeque<(i64, u64)>,
    backlog: u64,
}

impl Bucket {
    fn new(rate: Rate) -> Self {
        Self {
            rate,
            credit: i128::from(rate.burst_bytes) * UNITS_PER_BYTE,
            at_ns: i64::MIN,
            last_ns: i64::MIN,
            queue: VecDeque::new(),
            backlog: 0,
        }
    }

    fn cap(&self) -> i128 {
        i128::from(self.rate.burst_bytes) * UNITS_PER_BYTE
    }

    /// The credit at `t >= at_ns`.
    fn credit_at(&self, t: i64) -> i128 {
        if self.at_ns == i64::MIN {
            return self.credit;
        }
        let gained = i128::from(t - self.at_ns) * i128::from(self.rate.rate_bps);
        (self.credit + gained).min(self.cap())
    }

    /// The departure time of `bytes` arriving at `t`, or `None` for a tail drop.
    fn admit(&mut self, t: i64, bytes: u64) -> Option<i64> {
        while self.queue.front().is_some_and(|(d, _)| *d <= t) {
            if let Some((_, b)) = self.queue.pop_front() {
                self.backlog -= b;
            }
        }
        if self.backlog + bytes > self.rate.queue_bytes {
            return None;
        }
        let start = t.max(self.last_ns);
        let have = self.credit_at(start);
        let need = i128::from(bytes) * UNITS_PER_BYTE;
        let threshold = need.min(self.cap());
        let rate = i128::from(self.rate.rate_bps);
        let depart = if have >= threshold {
            start
        } else {
            let wait = (threshold - have + rate - 1) / rate;
            start + i64::try_from(wait).unwrap_or(i64::MAX)
        };
        let at_depart = if self.at_ns == i64::MIN && depart == start {
            have
        } else {
            (have + i128::from(depart - start) * rate).min(self.cap())
        };
        self.credit = at_depart - need;
        self.at_ns = depart;
        self.last_ns = depart;
        if depart > t {
            self.queue.push_back((depart, bytes));
            self.backlog += bytes;
        }
        Some(depart)
    }
}

/// The Gilbert–Elliott state.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
enum GeState {
    Good,
    Bad,
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
    last_send_ns: i64,
    /// The delivery time of the last message that was not reordered.
    last_fifo_ns: i64,
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
            last_send_ns: i64::MIN,
            last_fifo_ns: i64::MIN,
            spec,
        })
    }

    /// The parameters this link was built from.
    #[must_use]
    pub fn spec(&self) -> &LinkSpec {
        &self.spec
    }

    /// EMU-5, EMU-6: whether the loss stage drops this message. Its draws are
    /// made whatever the outcome.
    fn lost(&mut self) -> bool {
        let (Some(loss), Some(rng)) = (self.spec.loss, self.loss_rng.as_mut()) else {
            return false;
        };
        match loss {
            Loss::Iid { loss_ppm } => chance(rng, loss_ppm),
            Loss::GilbertElliott {
                p_good_bad_ppm,
                p_bad_good_ppm,
                loss_good_ppm,
                loss_bad_ppm,
            } => {
                let flip = below(rng, PPM);
                let drop = below(rng, PPM);
                self.ge = match self.ge {
                    GeState::Good if flip < p_good_bad_ppm => GeState::Bad,
                    GeState::Bad if flip < p_bad_good_ppm => GeState::Good,
                    s => s,
                };
                let p = match self.ge {
                    GeState::Good => loss_good_ppm,
                    GeState::Bad => loss_bad_ppm,
                };
                drop < p
            }
        }
    }
}

impl LinkModel for Link {
    fn transmit(&mut self, send_ns: i64, bytes: u64) -> Result<Fate, LinkError> {
        if send_ns < self.last_send_ns {
            return refuse(
                "order",
                format!(
                    "a message sent at {send_ns} ns follows one sent at {} ns",
                    self.last_send_ns
                ),
            );
        }
        self.last_send_ns = send_ns;
        let mut fate = Fate::new(send_ns);
        let mut t = send_ns;

        // Outage (EMU-7).
        if let Some(w) = self
            .spec
            .outage
            .as_ref()
            .and_then(|ws| ws.iter().find(|w| w.start_ns <= t && t < w.end_ns))
        {
            match w.mode {
                OutageMode::Drop => return Ok(fate.dropped(DropCause::Outage)),
                OutageMode::Hold => {
                    fate.hold_ns = w.end_ns - t;
                    t = w.end_ns;
                }
            }
        }

        // Loss (EMU-5, EMU-6).
        if self.lost() {
            return Ok(fate.dropped(DropCause::Loss));
        }

        // Rate (EMU-4).
        if let Some(bucket) = self.bucket.as_mut() {
            let Some(depart) = bucket.admit(t, bytes) else {
                return Ok(fate.dropped(DropCause::Queue));
            };
            fate.rate_wait_ns = depart - t;
            t = depart;
        }

        // Delay (EMU-3) and reorder (EMU-8).
        let mut deliver = t;
        if let Some(d) = self.spec.delay {
            let j = match self.delay_rng.as_mut() {
                Some(rng) => {
                    let span = u64::try_from(2 * d.jitter_ns + 1).unwrap_or(u64::MAX);
                    i64::try_from(below(rng, span)).unwrap_or(0) - d.jitter_ns
                }
                None => 0,
            };
            deliver = (t + d.delay_ns + j).max(t);
        }
        let reordered = match (self.spec.reorder, self.reorder_rng.as_mut()) {
            (Some(r), Some(rng)) => chance(rng, r.reorder_ppm),
            _ => false,
        };
        if let (true, Some(r)) = (reordered, self.spec.reorder) {
            deliver += r.gap_ns;
        } else {
            deliver = deliver.max(self.last_fifo_ns);
            self.last_fifo_ns = deliver;
        }
        fate.reordered = reordered;
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
        // Two 100-byte messages queue (100 ms apart); a third would exceed 300
        // bytes with the one that departs at 0 already gone.
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
}
