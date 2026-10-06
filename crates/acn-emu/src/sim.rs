//! The sim engine (SPEC 020 §4, EMU-30 to EMU-35, EMU-38): an event queue on
//! virtual time, and a network that carries calls across a scenario's paths.
//! A call's request crosses the path's uplink when it is made; its response
//! messages are registered when the server answers and offered to the downlink
//! as the network's clock reaches each one's send time, so the messages of
//! concurrent calls meet the link in time order (EMU-1).

use std::collections::BTreeMap;

use crate::link::{Direction, Fate, Link, LinkError, LinkModel as _};
use crate::scenario::Scenario;

/// Why the engine refused something.
#[derive(Debug, Clone, PartialEq, Eq, thiserror::Error)]
#[error("{reason}: {message}")]
pub struct SimError {
    /// `past` (an event before the clock), `path` (a path that is missing or
    /// lacks a direction), `call` (an unknown or repeated call) or a link's
    /// reason (EMU-1).
    pub reason: &'static str,
    pub message: String,
}

fn refuse<T>(reason: &'static str, message: impl Into<String>) -> Result<T, SimError> {
    Err(SimError {
        reason,
        message: message.into(),
    })
}

impl From<LinkError> for SimError {
    fn from(e: LinkError) -> Self {
        Self {
            reason: e.reason,
            message: e.message,
        }
    }
}

/// Events ordered by `(time, seq)` on a clock that only moves forward
/// (EMU-30, EMU-31).
#[derive(Debug, Clone)]
pub struct EventQueue<T> {
    now: i64,
    seq: u64,
    events: BTreeMap<(i64, u64), T>,
}

impl<T> Default for EventQueue<T> {
    fn default() -> Self {
        Self {
            now: 0,
            seq: 0,
            events: BTreeMap::new(),
        }
    }
}

impl<T> EventQueue<T> {
    /// An empty queue with its clock at 0.
    #[must_use]
    pub fn new() -> Self {
        Self::default()
    }

    /// The clock.
    #[must_use]
    pub fn now(&self) -> i64 {
        self.now
    }

    /// Add `item` at `time_ns`, refused when that is before the clock; returns
    /// its sequence number.
    pub fn push(&mut self, time_ns: i64, item: T) -> Result<u64, SimError> {
        if time_ns < self.now {
            return refuse(
                "past",
                format!(
                    "an event at {time_ns} ns is before the clock at {} ns",
                    self.now
                ),
            );
        }
        let seq = self.seq;
        self.seq += 1;
        self.events.insert((time_ns, seq), item);
        Ok(seq)
    }

    /// The time of the earliest pending event.
    #[must_use]
    pub fn next_time(&self) -> Option<i64> {
        self.events.keys().next().map(|(t, _)| *t)
    }

    /// Whether no event is pending.
    #[must_use]
    pub fn is_empty(&self) -> bool {
        self.events.is_empty()
    }

    /// Move the clock to the earliest pending time and hand out every event
    /// due then, in `seq` order (EMU-31).
    pub fn pop_group(&mut self) -> Option<(i64, Vec<T>)> {
        let t = self.next_time()?;
        self.now = t;
        let mut group = Vec::new();
        while let Some(entry) = self.events.first_entry() {
            if entry.key().0 != t {
                break;
            }
            group.push(entry.remove());
        }
        Some((t, group))
    }

    /// Move the clock forward to `t_ns` without handing anything out; refused
    /// when an event is pending before `t_ns`.
    pub fn advance_to(&mut self, t_ns: i64) -> Result<(), SimError> {
        if let Some(n) = self.next_time().filter(|n| *n < t_ns) {
            return refuse(
                "past",
                format!("an event at {n} ns is pending before {t_ns} ns"),
            );
        }
        self.now = self.now.max(t_ns);
        Ok(())
    }
}

/// A path: the `up` and `down` links of one name (EMU-32).
#[derive(Debug, Clone)]
pub struct Path {
    pub up: Link,
    pub down: Link,
}

/// One response message, as the network offered it to the downlink.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub struct Received {
    pub call: u64,
    /// The message's position in its response.
    pub index: usize,
    pub fate: Fate,
    /// When it was received under the order rule (EMU-34); `None` if dropped.
    pub received_ns: Option<i64>,
}

/// How a call's response ended (EMU-35).
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum CallOutcome {
    /// Every message arrived; the response was received at this time.
    Received(i64),
    /// A streamed response lost an event: the messages before it arrived, and
    /// the stream broke when the next event would have been received.
    Cut { first_lost: usize, at_ns: i64 },
    /// Nothing more will arrive: the call ends at its deadline.
    Lost,
}

/// A response being carried.
#[derive(Debug, Clone)]
struct Response {
    path: String,
    count: usize,
    received: Vec<Option<Received>>,
    last_received_ns: Option<i64>,
}

/// The network of one replicate: a scenario's paths and the response messages
/// in flight (EMU-32 to EMU-35).
#[derive(Debug, Clone)]
pub struct Network {
    paths: BTreeMap<String, Path>,
    pending: EventQueue<(u64, usize, u64)>,
    responses: BTreeMap<u64, Response>,
    requests: BTreeMap<u64, Fate>,
}

impl Network {
    /// The network of `scenario` for one replicate: each name's two links,
    /// built under `replicate_seed`. A name without both directions is refused.
    pub fn new(scenario: &Scenario, replicate_seed: u64) -> Result<Self, SimError> {
        let links = scenario.build(replicate_seed).map_err(|e| SimError {
            reason: e.reason,
            message: e.message,
        })?;
        let mut ups = BTreeMap::new();
        let mut downs = BTreeMap::new();
        for l in links {
            let name = l.spec().name.clone();
            match l.spec().direction {
                Direction::Up => ups.insert(name, l),
                Direction::Down => downs.insert(name, l),
            };
        }
        let mut paths = BTreeMap::new();
        for (name, up) in ups {
            let Some(down) = downs.remove(&name) else {
                return refuse("path", format!("path {name} has no down link"));
            };
            paths.insert(name, Path { up, down });
        }
        if let Some(name) = downs.keys().next() {
            return refuse("path", format!("path {name} has no up link"));
        }
        Ok(Self {
            paths,
            pending: EventQueue::new(),
            responses: BTreeMap::new(),
            requests: BTreeMap::new(),
        })
    }

    /// The names of the paths.
    pub fn paths(&self) -> impl Iterator<Item = &str> {
        self.paths.keys().map(String::as_str)
    }

    fn path(&mut self, name: &str) -> Result<&mut Path, SimError> {
        self.paths.get_mut(name).ok_or_else(|| SimError {
            reason: "path",
            message: format!("no path named {name}"),
        })
    }

    /// Send call `call`'s request of `bytes` on `path`'s uplink at `send_ns`
    /// (EMU-32); its fate says when the server sees it (EMU-33). Requests are
    /// sent in non-decreasing time (EMU-1).
    pub fn request(
        &mut self,
        call: u64,
        path: &str,
        send_ns: i64,
        bytes: u64,
    ) -> Result<Fate, SimError> {
        if self.requests.contains_key(&call) {
            return refuse("call", format!("call {call} already sent its request"));
        }
        let fate = self.path(path)?.up.transmit(send_ns, bytes)?;
        self.requests.insert(call, fate);
        Ok(fate)
    }

    /// Register call `call`'s response on `path`'s downlink: one message per
    /// `(send_ns, bytes)`, in order (EMU-32). Each is offered when the clock
    /// reaches its send time.
    pub fn respond(
        &mut self,
        call: u64,
        path: &str,
        messages: &[(i64, u64)],
    ) -> Result<(), SimError> {
        self.path(path)?;
        if messages.is_empty() {
            return refuse("call", format!("call {call}'s response has no message"));
        }
        if self.responses.contains_key(&call) {
            return refuse("call", format!("call {call} already has a response"));
        }
        for (i, (t, b)) in messages.iter().enumerate() {
            self.pending.push(*t, (call, i, *b))?;
        }
        self.responses.insert(
            call,
            Response {
                path: path.to_owned(),
                count: messages.len(),
                received: vec![None; messages.len()],
                last_received_ns: None,
            },
        );
        Ok(())
    }

    /// The send time of the next response message to offer.
    #[must_use]
    pub fn next_time(&self) -> Option<i64> {
        self.pending.next_time()
    }

    /// Offer every response message with a send time up to `t_ns` to its
    /// downlink, in `(time, seq)` order, and return them with their receive
    /// times (EMU-32, EMU-34).
    pub fn advance(&mut self, t_ns: i64) -> Result<Vec<Received>, SimError> {
        let mut out = Vec::new();
        while self.pending.next_time().is_some_and(|n| n <= t_ns) {
            let Some((t, group)) = self.pending.pop_group() else {
                break;
            };
            for (call, index, bytes) in group {
                let Some(path) = self.responses.get(&call).map(|r| r.path.clone()) else {
                    return refuse("call", format!("call {call} has no response"));
                };
                let fate = self.path(&path)?.down.transmit(t, bytes)?;
                let Some(r) = self.responses.get_mut(&call) else {
                    return refuse("call", format!("call {call} has no response"));
                };
                let received_ns = fate
                    .outcome
                    .ok()
                    .map(|d| r.last_received_ns.map_or(d, |p| d.max(p)));
                if let Some(at) = received_ns {
                    r.last_received_ns = Some(at);
                }
                let rec = Received {
                    call,
                    index,
                    fate,
                    received_ns,
                };
                r.received[index] = Some(rec);
                out.push(rec);
            }
        }
        self.pending.advance_to(t_ns)?;
        Ok(out)
    }

    /// How call `call` ended, once its request and every response message
    /// have been carried (EMU-35); `None` until then.
    #[must_use]
    pub fn outcome(&self, call: u64) -> Option<CallOutcome> {
        let req = self.requests.get(&call)?;
        if req.outcome.is_err() {
            return Some(CallOutcome::Lost);
        }
        let r = self.responses.get(&call)?;
        let got: Vec<&Received> = r
            .received
            .iter()
            .map(Option::as_ref)
            .collect::<Option<Vec<_>>>()?;
        match got.iter().position(|m| m.received_ns.is_none()) {
            None => got
                .last()
                .and_then(|m| m.received_ns)
                .map(CallOutcome::Received),
            Some(first_lost) => Some(
                got[first_lost..]
                    .iter()
                    .find_map(|m| m.received_ns)
                    .filter(|_| r.count > 1)
                    .map_or(CallOutcome::Lost, |at_ns| CallOutcome::Cut {
                        first_lost,
                        at_ns,
                    }),
            ),
        }
    }
}
