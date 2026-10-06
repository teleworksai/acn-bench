//! Where calls go and how time passes. An [`Env`] makes one attempt at a call and
//! waits on the run's clock (HAR-40). [`SimEnv`] is `sim`: the in-process mock on a
//! [`SimClock`], driven by a small deterministic scheduler — every lineage's
//! pending wait or call is registered with it, and calls due at the same instant
//! go to the mock together through `Mock::handle_batch`, so their order is MLM-7's
//! and never a scheduler's (HAR-41). Waits sit on the sim engine's event
//! queue (SPEC 020 EMU-30, EMU-31), which T11.3's network shares. [`LiveEnv`] is `live`: HTTP on the wall clock.

use std::cell::RefCell;
use std::collections::BTreeMap;
use std::future::Future;
use std::pin::Pin;
use std::rc::Rc;
use std::sync::Arc;
use std::task::{Context, Poll, Waker};

use acn_emu::clock::{Clock, SimClock, WallClock};
use acn_emu::sim::EventQueue;
use acn_mockllm::{Mock, Outcome};

use crate::HarnessError;
use crate::wire::{Exchange, Failure, LinkRecord, sse_events};

/// One attempt at a call, and the clock (HAR-24, HAR-33, HAR-40).
pub trait Env {
    /// Nanoseconds on the run's clock.
    fn now(&self) -> i64;
    /// Resolve once the run's clock reads at least `t_ns`.
    fn sleep_until(&self, t_ns: i64) -> impl Future<Output = ()>;
    /// POST `body` to `path` once; give up after `timeout_ns`.
    fn exchange(
        &self,
        path: &'static str,
        body: Vec<u8>,
        stream: bool,
        timeout_ns: i64,
    ) -> impl Future<Output = Exchange>;
}

// ---- sim -------------------------------------------------------------------

enum Op {
    Sleep(i64),
    Call {
        body: Vec<u8>,
        stream: bool,
        timeout_ns: i64,
    },
}

enum Done {
    Woke,
    Called(Box<Outcome>),
    /// With a scenario: the attempt's exchange, already on the network's
    /// receive times (SPEC 020 EMU-34, EMU-35).
    Carried(Box<Exchange>),
}

/// A call made at the current instant.
struct Pending {
    id: u64,
    body: Vec<u8>,
    stream: bool,
    timeout_ns: i64,
}

struct SimState {
    clock: SimClock,
    mock: Mock,
    tenant: String,
    next: u64,
    /// Pending waits, by due time then registration (EMU-30).
    waits: EventQueue<u64>,
    /// Calls registered at the current instant, in registration order.
    calls: Vec<Pending>,
    /// With a scenario, the network the calls cross (SPEC 020 §4).
    net: Option<crate::net::NetState>,
    done: BTreeMap<u64, Done>,
    /// A wait the queue refused, reported by `drive`. Unreachable today: a wait
    /// is registered only for `t > clock.now`, and the queue's own now (the
    /// last group it handed out) never passes the clock, which `drive` moves
    /// only to the queue's next time.
    fault: Option<String>,
}

/// The `sim` environment of one replicate: its mock, its tenant and its clock.
#[derive(Clone)]
pub struct SimEnv(Rc<RefCell<SimState>>);

impl std::fmt::Debug for SimEnv {
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        f.write_str("SimEnv")
    }
}

/// A wait or call registered with the scheduler.
struct OpFuture {
    env: SimEnv,
    op: Option<Op>,
    id: Option<u64>,
}

impl Future for OpFuture {
    type Output = Done;

    fn poll(mut self: Pin<&mut Self>, _cx: &mut Context<'_>) -> Poll<Done> {
        let this = &mut *self;
        let mut st = this.env.0.borrow_mut();
        match this.id {
            None => {
                let id = st.next;
                st.next += 1;

                match this.op.take() {
                    Some(Op::Sleep(t)) => {
                        if let Err(e) = st.waits.push(t, id) {
                            st.fault = Some(e.to_string());
                        }
                    }
                    Some(Op::Call {
                        body,
                        stream,
                        timeout_ns,
                    }) => st.calls.push(Pending {
                        id,
                        body,
                        stream,
                        timeout_ns,
                    }),
                    None => {}
                }
                this.id = Some(id);
                Poll::Pending
            }
            Some(id) => st.done.remove(&id).map_or(Poll::Pending, Poll::Ready),
        }
    }
}

impl SimEnv {
    /// A replicate's environment: a fresh clock at 0, its own mock, its tenant
    /// (HAR-42: the isolation marker).
    #[must_use]
    pub fn new(mock: Mock, tenant: String) -> Self {
        Self(Rc::new(RefCell::new(SimState {
            clock: SimClock::new(),
            mock,
            tenant,
            next: 0,
            waits: EventQueue::new(),
            calls: Vec::new(),
            net: None,
            done: BTreeMap::new(),
            fault: None,
        })))
    }

    /// A replicate's environment whose calls cross `scenario`'s one path,
    /// built under the replicate seed (SPEC 020 EMU-32).
    pub fn with_scenario(
        mock: Mock,
        tenant: String,
        scenario: &acn_emu::scenario::Scenario,
        replicate_seed: u64,
    ) -> Result<Self, HarnessError> {
        let net = crate::net::NetState::new(scenario, replicate_seed)
            .map_err(|e| HarnessError::Config(format!("scenario: {e}")))?;
        let env = Self::new(mock, tenant);
        env.0.borrow_mut().net = Some(net);
        Ok(env)
    }

    /// Every fate the network decided, in order; empty without a scenario.
    #[must_use]
    pub fn fates(&self) -> Vec<(acn_emu::link::Direction, acn_emu::link::Fate)> {
        self.0
            .borrow()
            .net
            .as_ref()
            .map_or_else(Vec::new, |n| n.fates().to_vec())
    }

    /// The mock's cache sizes, for tests.
    #[must_use]
    pub fn cache_sizes(&self) -> (usize, usize) {
        self.0.borrow().mock.cache_sizes()
    }

    /// Run `fut` to completion on virtual time. Each round wakes the waits that
    /// are due, then hands every call registered at this instant to the mock as
    /// one batch, and only when neither is left moves the clock to the next wait.
    pub fn drive<F: Future>(&self, fut: F) -> Result<F::Output, HarnessError> {
        let mut fut = std::pin::pin!(fut);
        let mut cx = Context::from_waker(Waker::noop());
        loop {
            if let Poll::Ready(v) = fut.as_mut().poll(&mut cx) {
                return Ok(v);
            }
            let mut st = self.0.borrow_mut();
            if let Some(f) = st.fault.take() {
                return Err(HarnessError::Internal(format!("the sim scheduler: {f}")));
            }
            let now = st.clock.now_ns();
            if st.net.is_some() {
                if Self::network_step(&mut st, now)? {
                    continue;
                }
                return Err(HarnessError::Internal(
                    "the sim scheduler has nothing to run and the run has not finished".into(),
                ));
            }
            // Wake every wait due now (EMU-31: one group, in registration order).
            if st.waits.next_time().is_some_and(|t| t <= now) {
                if let Some((_, ids)) = st.waits.pop_group() {
                    for id in ids {
                        st.done.insert(id, Done::Woke);
                    }
                }
                continue;
            }
            // Then hand every call made at this instant to the mock as one batch.
            if !st.calls.is_empty() {
                let calls = std::mem::take(&mut st.calls);
                let tenant = st.tenant.clone();
                let requests: Vec<(Vec<u8>, String, i64)> = calls
                    .iter()
                    .map(|c| (c.body.clone(), tenant.clone(), now))
                    .collect();
                let outcomes = st.mock.handle_batch(&requests);
                for (c, o) in calls.iter().zip(outcomes) {
                    st.done.insert(c.id, Done::Called(Box::new(o)));
                }
                continue;
            }
            // Only then move the clock to the next wait.
            match st.waits.next_time() {
                Some(t) => st.clock.advance_to(t),
                None => {
                    return Err(HarnessError::Internal(
                        "the sim scheduler has nothing to run and the run has not finished".into(),
                    ));
                }
            }
        }
    }

    /// One step of an instant with a network (SPEC 020 EMU-33): wake the waits
    /// due, then end the attempts whose end is known, then send the requests
    /// made at this instant up the link, then hand the requests delivered now to
    /// the mock as one batch; only when none of that is left, move the clock.
    /// Returns whether anything happened.
    fn network_step(st: &mut SimState, now: i64) -> Result<bool, HarnessError> {
        let sim = |e: acn_emu::sim::SimError| HarnessError::Internal(format!("the network: {e}"));
        // Waits due now and attempts that end now resume together, as one
        // group: without a network an attempt's end is itself a wait, so this
        // keeps the order in which lineages resume, and draw, the same.
        let mut woke = false;
        if st.waits.next_time().is_some_and(|t| t <= now)
            && let Some((_, ids)) = st.waits.pop_group()
        {
            for id in ids {
                st.done.insert(id, Done::Woke);
            }
            woke = true;
        }
        let Some(net) = st.net.as_mut() else {
            return Ok(woke);
        };
        let settled = net.settle(now).map_err(sim)?;
        for (id, ex) in settled.resolved {
            st.done.insert(id, Done::Carried(Box::new(ex)));
            woke = true;
        }
        if woke {
            return Ok(true);
        }
        if !st.calls.is_empty() {
            for c in std::mem::take(&mut st.calls) {
                net.send(c.id, now, &c.body, c.stream, c.timeout_ns)
                    .map_err(sim)?;
            }
            return Ok(true);
        }
        if net.arriving(now) {
            // EMU-33: the requests delivered now reach the mock as one batch,
            // at this instant; their order is MLM-7's.
            let arrived = net.take_arrivals(now);
            let requests: Vec<(Vec<u8>, String, i64)> = arrived
                .iter()
                .map(|(_, b)| (b.clone(), st.tenant.clone(), now))
                .collect();
            let outcomes = st.mock.handle_batch(&requests);
            for ((id, _), o) in arrived.iter().zip(outcomes) {
                net.answer(*id, o).map_err(sim)?;
            }
            return Ok(true);
        }
        match [st.waits.next_time(), net.next_time()]
            .into_iter()
            .flatten()
            .min()
        {
            Some(t) if t > now => {
                st.clock.advance_to(t);
                Ok(true)
            }
            _ => Ok(false),
        }
    }

    fn op(&self, op: Op) -> OpFuture {
        OpFuture {
            env: self.clone(),
            op: Some(op),
            id: None,
        }
    }
}

fn sse_len(data: &str) -> u64 {
    u64::try_from("data: ".len() + data.len() + 2).unwrap_or(u64::MAX)
}

impl Env for SimEnv {
    fn now(&self) -> i64 {
        self.0.borrow().clock.now_ns()
    }

    async fn sleep_until(&self, t_ns: i64) {
        if t_ns > self.now() {
            self.op(Op::Sleep(t_ns)).await;
        }
    }

    async fn exchange(
        &self,
        _path: &'static str,
        body: Vec<u8>,
        stream: bool,
        timeout_ns: i64,
    ) -> Exchange {
        let start = self.now();
        let o = match self
            .op(Op::Call {
                body,
                stream,
                timeout_ns,
            })
            .await
        {
            Done::Called(o) => o,
            // With a scenario the exchange is already on receive times; the
            // attempt waits for its end (SPEC 020 EMU-34, EMU-35).
            Done::Carried(ex) => {
                self.sleep_until(ex.end_ns).await;
                return *ex;
            }
            Done::Woke => {
                return Exchange {
                    start_ns: start,
                    end_ns: start,
                    failure: Some(Failure::Transport(
                        "internal: a call was woken as a wait".into(),
                    )),
                    ..Exchange::default()
                };
            }
        };
        let o = *o;
        let streamed = stream && o.status == 200;
        let end = if o.status != 200 {
            start
        } else if streamed {
            o.chunks.last().map_or(start, |c| c.at_ns)
        } else {
            o.respond_at_ns
        };
        let deadline = start.saturating_add(timeout_ns);
        if end > deadline {
            self.sleep_until(deadline).await;
            return Exchange {
                start_ns: start,
                end_ns: deadline,
                status: o.status,
                failure: Some(Failure::Timeout),
                ..Exchange::default()
            };
        }
        self.sleep_until(end).await;
        let (events, body, bytes_down) = if streamed {
            let events: Vec<(i64, String)> =
                o.chunks.iter().map(|c| (c.at_ns, c.data.clone())).collect();
            let n = events.iter().map(|(_, d)| sse_len(d)).sum();
            (events, Vec::new(), n)
        } else {
            let n = u64::try_from(o.body.len()).unwrap_or(u64::MAX);
            (Vec::new(), o.body, n)
        };
        Exchange {
            start_ns: start,
            end_ns: end,
            status: o.status,
            headers: o.headers,
            events,
            body,
            bytes_down,
            failure: None,
            links: Vec::new(),
        }
    }
}

// ---- live ------------------------------------------------------------------

/// The `live` environment: one endpoint over HTTP, on the wall clock.
#[derive(Clone)]
pub struct LiveEnv {
    client: reqwest::Client,
    clock: Arc<WallClock>,
    endpoint: String,
    headers: Vec<(String, String)>,
    /// With a scenario, the replicate's proxy (SPEC 020 §5).
    net: Option<LiveNet>,
}

/// A live replicate's proxy, its origin on the run's clock, and the next
/// attempt's tag (EMU-47), shared by every clone of the environment.
#[derive(Clone)]
struct LiveNet {
    proxy: Arc<acn_emu::proxy::Proxy>,
    origin_ns: i64,
    next: Arc<std::sync::atomic::AtomicU64>,
}

impl std::fmt::Debug for LiveEnv {
    /// Never the headers: they carry credentials (HAR-22).
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        f.debug_struct("LiveEnv")
            .field("endpoint", &self.endpoint)
            .field(
                "headers",
                &format_args!("<{} redacted>", self.headers.len()),
            )
            .finish_non_exhaustive()
    }
}

impl LiveEnv {
    /// An environment that sends `headers` (credentials, the tenant) with every
    /// request to `endpoint` (a base URL, without `/v1/...`).
    pub fn new(
        clock: Arc<WallClock>,
        endpoint: &str,
        headers: Vec<(String, String)>,
    ) -> Result<Self, HarnessError> {
        Ok(Self::with_client(
            Self::http_client()?,
            clock,
            endpoint,
            headers,
        ))
    }

    /// [`LiveEnv::new`] over a client the run already built.
    #[must_use]
    pub fn with_client(
        client: reqwest::Client,
        clock: Arc<WallClock>,
        endpoint: &str,
        headers: Vec<(String, String)>,
    ) -> Self {
        Self {
            client,
            clock,
            endpoint: endpoint.trim_end_matches('/').to_owned(),
            headers,
            net: None,
        }
    }

    /// The HTTP client of a live run. Building one loads the system's root
    /// certificates, which takes a while, so a run builds it once.
    pub fn http_client() -> Result<reqwest::Client, HarnessError> {
        // No proxy from the environment: `HTTP_PROXY` and friends would change
        // what a run receives without changing its identity, and would receive
        // its credentials (CON-29, HAR-22, HAR-25).
        reqwest::Client::builder()
            .no_proxy()
            .redirect(reqwest::redirect::Policy::none())
            .build()
            .map_err(|e| HarnessError::Config(format!("http client: {e}")))
    }

    /// An environment whose calls cross `proxy` (SPEC 020 EMU-40), whose links
    /// count from `origin_ns` on the run's clock, over `client`. Every attempt
    /// is tagged, and carries the proxy's records of it (EMU-47).
    #[must_use]
    pub fn through_proxy(
        client: reqwest::Client,
        clock: Arc<WallClock>,
        proxy: Arc<acn_emu::proxy::Proxy>,
        origin_ns: i64,
        headers: Vec<(String, String)>,
    ) -> Self {
        Self {
            client,
            clock,
            endpoint: format!("http://{}", proxy.addr()),
            headers,
            net: Some(LiveNet {
                proxy,
                origin_ns,
                next: Arc::new(std::sync::atomic::AtomicU64::new(0)),
            }),
        }
    }

    /// The records of attempt `n` carried by its end at `end_ns` on the run's
    /// clock: received by then, or lost and sent by then (EMU-36, EMU-47).
    fn carried(net: &LiveNet, n: u64, end_ns: i64) -> Vec<LinkRecord> {
        let end = end_ns - net.origin_ns;
        net.proxy
            .records(n)
            .into_iter()
            .filter(|r| match (r.fate.outcome, r.received_ns) {
                (Ok(_), Some(t)) => t <= end,
                (Err(_), _) => r.fate.send_ns <= end,
                (Ok(_), None) => false,
            })
            .map(|r| LinkRecord {
                direction: r.direction,
                bytes: r.bytes,
                fate: r.fate,
                received_ns: r.received_ns,
            })
            .collect()
    }

    fn request(&self, method: reqwest::Method, path: &str) -> reqwest::RequestBuilder {
        let mut r = self
            .client
            .request(method, format!("{}{path}", self.endpoint));
        for (k, v) in &self.headers {
            r = r.header(k, v);
        }
        r
    }

    /// HAR-23: whether `GET /v1/models` shows the mock's marker.
    pub async fn probe_is_mock(&self) -> Result<bool, HarnessError> {
        let resp = self
            .request(reqwest::Method::GET, "/v1/models")
            .send()
            .await
            .map_err(|e| HarnessError::Backend(format!("GET /v1/models: {}", cause(e))))?;
        Ok(resp.headers().contains_key(crate::wire::MOCK_HEADER))
    }

    async fn attempt(
        &self,
        path: &'static str,
        body: Vec<u8>,
        stream: bool,
        start: i64,
        tag: Option<u64>,
    ) -> Exchange {
        use futures_util::StreamExt as _;
        let mut ex = Exchange {
            start_ns: start,
            ..Exchange::default()
        };
        let mut req = self
            .request(reqwest::Method::POST, path)
            .header("content-type", "application/json");
        if let Some(n) = tag {
            req = req.header(acn_emu::proxy::ATTEMPT_HEADER, n.to_string());
        }
        let resp = match req.body(body).send().await {
            Ok(r) => r,
            Err(e) => {
                ex.end_ns = self.clock.now_ns();
                ex.failure = Some(Failure::Transport(cause(e)));
                return ex;
            }
        };
        ex.status = resp.status().as_u16();
        ex.headers = resp
            .headers()
            .iter()
            .filter_map(|(k, v)| {
                v.to_str()
                    .ok()
                    .map(|v| (k.as_str().to_owned(), v.to_owned()))
            })
            .collect();
        if stream && ex.status == 200 {
            let mut s = resp.bytes_stream();
            let mut buf = Vec::new();
            while let Some(chunk) = s.next().await {
                match chunk {
                    Ok(bytes) => {
                        let at = self.clock.now_ns();
                        ex.bytes_down += u64::try_from(bytes.len()).unwrap_or(u64::MAX);
                        buf.extend_from_slice(&bytes);
                        let (events, used) = sse_events(&buf);
                        buf.drain(..used);
                        ex.events.extend(events.into_iter().map(|d| (at, d)));
                    }
                    Err(e) => {
                        ex.failure = Some(Failure::Transport(cause(e)));
                        break;
                    }
                }
            }
        } else {
            match resp.bytes().await {
                Ok(b) => {
                    ex.bytes_down = u64::try_from(b.len()).unwrap_or(u64::MAX);
                    ex.body = b.to_vec();
                }
                Err(e) => ex.failure = Some(Failure::Transport(cause(e))),
            }
        }
        ex.end_ns = self.clock.now_ns();
        ex
    }
}

/// A transport error with its causes (connection refused, DNS, TLS), without
/// the URL: reqwest's own text stops at "error sending request".
fn cause(e: reqwest::Error) -> String {
    let e = e.without_url();
    let mut text = e.to_string();
    let mut source = std::error::Error::source(&e);
    while let Some(s) = source {
        text.push_str(": ");
        text.push_str(&s.to_string());
        source = s.source();
    }
    text
}

impl Env for LiveEnv {
    fn now(&self) -> i64 {
        self.clock.now_ns()
    }

    async fn sleep_until(&self, t_ns: i64) {
        self.clock.sleep_until(t_ns).await;
    }

    async fn exchange(
        &self,
        path: &'static str,
        body: Vec<u8>,
        stream: bool,
        timeout_ns: i64,
    ) -> Exchange {
        let start = self.now();
        let deadline = start.saturating_add(timeout_ns);
        let tag = self
            .net
            .as_ref()
            .map(|n| n.next.fetch_add(1, std::sync::atomic::Ordering::SeqCst));
        let mut ex = tokio::select! {
            ex = self.attempt(path, body, stream, start, tag) => ex,
            () = self.clock.sleep_until(deadline) => Exchange {
                start_ns: start,
                end_ns: self.now(),
                failure: Some(Failure::Timeout),
                ..Exchange::default()
            },
        };
        if let (Some(net), Some(n)) = (&self.net, tag) {
            ex.links = Self::carried(net, n, ex.end_ns);
        }
        ex
    }
}

/// Run every future to completion, polling all of them each time, in order. The
/// sim scheduler's no-op waker never re-polls a single child, so a combinator that
/// waits for wake-ups would stall; this one does not, and it is deterministic.
pub async fn join_all<F: Future>(futs: Vec<F>) -> Vec<F::Output> {
    let mut futs: Vec<Pin<Box<F>>> = futs.into_iter().map(Box::pin).collect();
    let mut out: Vec<Option<F::Output>> = futs.iter().map(|_| None).collect();
    std::future::poll_fn(|cx| {
        let mut pending = false;
        for (f, slot) in futs.iter_mut().zip(out.iter_mut()) {
            if slot.is_none() {
                match f.as_mut().poll(cx) {
                    Poll::Ready(v) => *slot = Some(v),
                    Poll::Pending => pending = true,
                }
            }
        }
        if pending {
            Poll::Pending
        } else {
            Poll::Ready(())
        }
    })
    .await;
    out.into_iter().flatten().collect()
}
