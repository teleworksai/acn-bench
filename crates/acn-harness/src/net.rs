//! The network of a `sim` replicate with a scenario (SPEC 020 EMU-32 to
//! EMU-35): each attempt's request crosses the scenario's uplink, reaches the
//! mock at its delivery time, and its response crosses the downlink, message by
//! message. An attempt ends at the earliest of its last message's receipt, a
//! cut stream's next receipt, and its deadline (HAR-24), and the exchange it
//! returns carries receive times, not the mock's send times.

use std::collections::BTreeMap;

use acn_emu::link::{Direction, Fate};
use acn_emu::scenario::Scenario;
use acn_emu::sim::{EventQueue, Network, Received, SimError};
use acn_mockllm::Outcome;

use crate::wire::{Exchange, Failure, LinkRecord};

/// The bytes of one SSE event on the wire: `data: …` and the blank line.
pub(crate) fn sse_len(data: &str) -> u64 {
    u64::try_from("data: ".len() + data.len() + 2).unwrap_or(u64::MAX)
}

/// An attempt on the network.
struct Inflight {
    start_ns: i64,
    deadline_ns: i64,
    stream: bool,
    up: LinkRecord,
    /// The request body, until the request reaches the mock.
    body: Option<Vec<u8>>,
    /// The mock's answer, once the request arrived.
    answer: Option<Outcome>,
}

/// The network part of a `SimEnv`.
pub(crate) struct NetState {
    net: Network,
    path: String,
    /// Requests in flight, by delivery time (EMU-33).
    arrivals: EventQueue<u64>,
    inflight: BTreeMap<u64, Inflight>,
    /// Every fate, in the order the network decided it, for EMU-37.
    fates: Vec<(Direction, Fate)>,
}

/// What an instant's step of the network did.
pub(crate) struct Settled {
    pub resolved: Vec<(u64, Exchange)>,
}

impl NetState {
    /// The network of `scenario` for one replicate (EMU-32: one path).
    pub(crate) fn new(scenario: &Scenario, replicate_seed: u64) -> Result<Self, SimError> {
        let net = Network::new(scenario, replicate_seed)?;
        let paths: Vec<String> = net.paths().map(str::to_owned).collect();
        let [path] = paths.as_slice() else {
            return Err(SimError {
                reason: "path",
                message: format!(
                    "a scenario for a run has exactly one path, not {}",
                    paths.len()
                ),
            });
        };
        Ok(Self {
            path: path.clone(),
            net,
            arrivals: EventQueue::new(),
            inflight: BTreeMap::new(),
            fates: Vec::new(),
        })
    }

    /// Every fate the network decided, in order.
    pub(crate) fn fates(&self) -> &[(Direction, Fate)] {
        &self.fates
    }

    /// Send attempt `id`'s request at `now` (EMU-32).
    pub(crate) fn send(
        &mut self,
        id: u64,
        now: i64,
        body: &[u8],
        stream: bool,
        timeout_ns: i64,
    ) -> Result<(), SimError> {
        let bytes = u64::try_from(body.len()).unwrap_or(u64::MAX);
        let fate = self.net.request(id, &self.path, now, bytes)?;
        self.fates.push((Direction::Up, fate));
        if let Ok(at) = fate.outcome {
            self.arrivals.push(at, id)?;
        }
        self.inflight.insert(
            id,
            Inflight {
                start_ns: now,
                deadline_ns: now.saturating_add(timeout_ns.max(0)),
                stream,
                up: LinkRecord {
                    direction: Direction::Up,
                    bytes,
                    fate,
                    received_ns: fate.outcome.ok(),
                },
                body: fate.outcome.is_ok().then(|| body.to_vec()),
                answer: None,
            },
        );
        Ok(())
    }

    /// Whether requests arrive at `now`.
    pub(crate) fn arriving(&self, now: i64) -> bool {
        self.arrivals.next_time().is_some_and(|t| t <= now)
    }

    /// The requests that arrive at `now`, with their bodies, in the order they
    /// were sent.
    pub(crate) fn take_arrivals(&mut self, now: i64) -> Vec<(u64, Vec<u8>)> {
        let ids = match self.arrivals.next_time() {
            Some(t) if t <= now => self.arrivals.pop_group().map_or_else(Vec::new, |(_, g)| g),
            _ => Vec::new(),
        };
        ids.into_iter()
            .filter_map(|id| {
                let body = self.inflight.get_mut(&id)?.body.take()?;
                Some((id, body))
            })
            .collect()
    }

    /// Hand attempt `id`'s answer to the downlink (EMU-32): a streamed 200 as
    /// one message per event at its emission time, anything else as one
    /// message of its body at the mock's response time.
    pub(crate) fn answer(&mut self, id: u64, o: Outcome) -> Result<(), SimError> {
        let stream = self.inflight.get(&id).is_some_and(|f| f.stream);
        let messages: Vec<(i64, u64)> = if stream && o.status == 200 && !o.chunks.is_empty() {
            o.chunks
                .iter()
                .map(|c| (c.at_ns, sse_len(&c.data)))
                .collect()
        } else {
            vec![(
                o.respond_at_ns,
                u64::try_from(o.body.len()).unwrap_or(u64::MAX),
            )]
        };
        self.net.respond(id, &self.path, &messages)?;
        if let Some(f) = self.inflight.get_mut(&id) {
            f.answer = Some(o);
        }
        Ok(())
    }

    /// The next time the network has something to do: an arrival, a response
    /// message to offer, or a deadline.
    pub(crate) fn next_time(&self) -> Option<i64> {
        [
            self.arrivals.next_time(),
            self.net.next_time(),
            self.inflight.values().map(|f| f.deadline_ns).min(),
        ]
        .into_iter()
        .flatten()
        .min()
    }

    /// Offer the response messages due by `now`, and end every attempt whose
    /// end is now known (EMU-34, EMU-35).
    pub(crate) fn settle(&mut self, now: i64) -> Result<Settled, SimError> {
        for r in self.net.advance(now)? {
            self.fates.push((Direction::Down, r.fate));
        }
        let mut resolved = Vec::new();
        let ids: Vec<u64> = self.inflight.keys().copied().collect();
        for id in ids {
            if let Some(ex) = self.resolve(id, now) {
                self.inflight.remove(&id);
                resolved.push((id, ex));
            }
        }
        Ok(Settled { resolved })
    }

    /// Attempt `id`'s exchange, if its end is known at `now`.
    fn resolve(&self, id: u64, now: i64) -> Option<Exchange> {
        let f = self.inflight.get(&id)?;
        let messages: &[Option<Received>] = self.net.messages(id).unwrap_or(&[]);
        // The end, if known: the last receipt, or a cut at the receipt after a
        // lost message; `None` while messages remain to be offered.
        let known = f.answer.as_ref().and_then(|_| end_of(messages));
        let ending = match known {
            Some(e) if e.at_ns() <= f.deadline_ns => e,
            Some(_) => End::Timeout,
            None if now >= f.deadline_ns => End::Timeout,
            None => return None,
        };
        Some(self.exchange(f, messages, ending))
    }

    fn exchange(&self, f: &Inflight, messages: &[Option<Received>], end: End) -> Exchange {
        let end_ns = match end {
            End::Received(t) | End::Cut(t) => t,
            End::Timeout => f.deadline_ns,
        };
        let mut links = vec![f.up];
        links.extend(messages.iter().flatten().map(|r| LinkRecord {
            direction: Direction::Down,
            bytes: 0,
            fate: r.fate,
            received_ns: r.received_ns,
        }));
        let mut ex = Exchange {
            start_ns: f.start_ns,
            end_ns,
            status: f.answer.as_ref().map_or(0, |o| o.status),
            failure: match end {
                End::Received(_) => None,
                End::Cut(_) => Some(Failure::Transport("the network cut the stream".into())),
                End::Timeout => Some(Failure::Timeout),
            },
            ..Exchange::default()
        };
        if let Some(o) = &f.answer {
            let streamed = f.stream && o.status == 200 && !o.chunks.is_empty();
            // The offered messages are a prefix of the response (its send times
            // do not decrease), so the k-th down record is message k.
            for (k, l) in links.iter_mut().skip(1).enumerate() {
                l.bytes = if streamed {
                    o.chunks.get(k).map_or(0, |c| sse_len(&c.data))
                } else {
                    u64::try_from(o.body.len()).unwrap_or(u64::MAX)
                };
            }
            // The messages received by the end, before any lost one.
            let got: Vec<(usize, i64)> = messages
                .iter()
                .enumerate()
                .map_while(|(k, m)| m.and_then(|r| r.received_ns).map(|t| (k, t)))
                .filter(|(_, t)| *t <= end_ns)
                .collect();
            if streamed {
                ex.events = got
                    .iter()
                    .filter_map(|(k, t)| o.chunks.get(*k).map(|c| (*t, c.data.clone())))
                    .collect();
                ex.bytes_down = ex.events.iter().map(|(_, d)| sse_len(d)).sum();
            } else if !got.is_empty() {
                ex.body.clone_from(&o.body);
                ex.bytes_down = u64::try_from(o.body.len()).unwrap_or(u64::MAX);
            }
            if matches!(end, End::Received(_)) {
                ex.headers.clone_from(&o.headers);
            }
        }
        ex.links = links;
        ex
    }
}

/// How a response's messages say it ended (EMU-35), once enough is known.
#[derive(Debug, Clone, Copy)]
enum End {
    Received(i64),
    Cut(i64),
    Timeout,
}

impl End {
    fn at_ns(self) -> i64 {
        match self {
            Self::Received(t) | Self::Cut(t) => t,
            Self::Timeout => i64::MAX,
        }
    }
}

/// The end of a response from its messages so far: every message received
/// (the last receipt), a lost message followed by a received one (a cut at
/// that receipt), every message offered with the last ones lost (a timeout),
/// or `None` while that is not yet decided.
fn end_of(messages: &[Option<Received>]) -> Option<End> {
    let mut lost = false;
    let mut last = None;
    for m in messages {
        let r = (*m)?;
        match r.received_ns {
            Some(t) if lost => return Some(End::Cut(t)),
            Some(t) => last = Some(t),
            None => lost = true,
        }
    }
    if lost {
        Some(End::Timeout)
    } else {
        last.map(End::Received)
    }
}
